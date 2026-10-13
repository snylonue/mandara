//! Atomic download starts and delivery of persisted content.

use mandara_core::model::Chapter;
use std::collections::{HashMap, hash_map::Entry};
use std::sync::{Arc, Mutex};
use tokio::sync::{Semaphore, oneshot};

#[derive(Clone, Debug, Hash, PartialEq, Eq)]
pub(super) enum CacheKey {
    Chapter(String, u32),
    BookFile(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub(super) enum DownloadError {
    #[error("content not found")]
    NotFound,
    #[error("content download failed")]
    Failed,
}

#[derive(Debug, Clone)]
pub(super) enum DownloadedContent {
    Chapter(Chapter),
    // All chapter indices share one immutable whole-book result.
    Book(Arc<[Chapter]>),
}

impl DownloadedContent {
    pub fn chapter(self, idx: u32) -> Result<Chapter, DownloadError> {
        match self {
            Self::Chapter(chapter) if chapter.idx == idx => Ok(chapter),
            Self::Book(chapters) => chapters
                .get(idx as usize)
                .cloned()
                .ok_or(DownloadError::NotFound),
            _ => Err(DownloadError::NotFound),
        }
    }
}

pub(super) type DownloadResult = Result<DownloadedContent, DownloadError>;
type Completion = oneshot::Sender<DownloadResult>;

pub(super) struct Downloads {
    waiters: Mutex<HashMap<CacheKey, Vec<Completion>>>,
    pub permits: Semaphore,
}

impl Default for Downloads {
    fn default() -> Self {
        Self {
            waiters: Mutex::new(HashMap::new()),
            permits: Semaphore::new(2),
        }
    }
}

impl Downloads {
    /// Only the first caller starts a writer; all callers receive its result.
    pub fn join(&self, key: CacheKey) -> (oneshot::Receiver<DownloadResult>, bool) {
        let (sender, receiver) = oneshot::channel();
        let mut waiters = self.waiters.lock().expect("download waiters poisoned");
        let start = match waiters.entry(key) {
            Entry::Vacant(entry) => {
                entry.insert(vec![sender]);
                true
            }
            Entry::Occupied(mut entry) => {
                entry.get_mut().push(sender);
                false
            }
        };
        (receiver, start)
    }

    /// Deliver content only after its cache write has finished, without holding a lock during I/O.
    pub fn complete(&self, key: &CacheKey, result: DownloadResult) {
        let waiters = self
            .waiters
            .lock()
            .expect("download waiters poisoned")
            .remove(key)
            .unwrap_or_default();
        for waiter in waiters {
            let _ = waiter.send(result.clone());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn content() -> DownloadedContent {
        DownloadedContent::Chapter(Chapter {
            idx: 0,
            title: "第一章".into(),
            content: "<p>内容</p>".into(),
        })
    }

    #[test]
    fn empty_or_blank_content_does_not_make_an_existing_chapter_missing() {
        for content in ["", " \n\t", "<p></p>"] {
            let chapter = Chapter {
                idx: 0,
                title: "第一章".into(),
                content: content.into(),
            };
            let single = DownloadedContent::Chapter(chapter.clone());
            assert_eq!(single.clone().chapter(0).unwrap().content, content);
            assert_eq!(single.chapter(1).unwrap_err(), DownloadError::NotFound);
            let book = DownloadedContent::Book(vec![chapter].into());
            assert_eq!(book.clone().chapter(0).unwrap().content, content);
            assert_eq!(book.chapter(1).unwrap_err(), DownloadError::NotFound);
        }
    }

    #[tokio::test]
    async fn starts_once_and_delivers_content_after_the_cache_write() {
        let downloads = Arc::new(Downloads::default());
        let key = CacheKey::Chapter("file".into(), 0);
        let (first, start) = downloads.join(key.clone());
        assert!(start);
        let (mut second, start) = downloads.join(key.clone());
        assert!(!start);
        let (canceled, start) = downloads.join(key.clone());
        assert!(!start);
        drop(canceled);
        assert!(matches!(
            second.try_recv(),
            Err(oneshot::error::TryRecvError::Empty)
        ));
        for other in [
            CacheKey::Chapter("file".into(), 1),
            CacheKey::Chapter("other".into(), 0),
            CacheKey::BookFile("file".into()),
        ] {
            let (done, start) = downloads.join(other.clone());
            assert!(start);
            downloads.complete(&other, Ok(content()));
            done.await.unwrap().unwrap();
        }
        let cache = Arc::new(AtomicUsize::new(0));
        let writer = {
            let downloads = downloads.clone();
            let cache = cache.clone();
            let key = key.clone();
            tokio::spawn(async move {
                cache.store(42, Ordering::Release);
                downloads.complete(&key, Ok(content()));
            })
        };
        for reader in [first, second] {
            assert_eq!(
                reader.await.unwrap().unwrap().chapter(0).unwrap().content,
                "<p>内容</p>"
            );
            assert_eq!(cache.load(Ordering::Acquire), 42);
        }
        writer.await.unwrap();
        assert!(downloads.waiters.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn concurrent_requests_start_exactly_one_writer() {
        let downloads = Arc::new(Downloads::default());
        let key = CacheKey::Chapter("file".into(), 0);
        let joined = Arc::new(tokio::sync::Barrier::new(17));
        let starts = Arc::new(AtomicUsize::new(0));
        let mut readers = Vec::new();
        for _ in 0..16 {
            let downloads = downloads.clone();
            let key = key.clone();
            let joined = joined.clone();
            let starts = starts.clone();
            readers.push(tokio::spawn(async move {
                let (completed, start) = downloads.join(key);
                if start {
                    starts.fetch_add(1, Ordering::Relaxed);
                }
                joined.wait().await;
                completed.await.unwrap().unwrap();
            }));
        }
        joined.wait().await;
        assert_eq!(starts.load(Ordering::Relaxed), 1);
        downloads.complete(&key, Ok(content()));
        for reader in readers {
            reader.await.unwrap();
        }
    }

    #[tokio::test]
    async fn failures_notify_waiters_and_a_new_request_can_start() {
        let downloads = Downloads::default();
        let key = CacheKey::BookFile("file".into());
        for error in [DownloadError::NotFound, DownloadError::Failed] {
            let (first, start) = downloads.join(key.clone());
            assert!(start);
            let (second, start) = downloads.join(key.clone());
            assert!(!start);
            downloads.complete(&key, Err(error));
            for waiter in [first, second] {
                assert_eq!(waiter.await.unwrap().unwrap_err(), error);
            }
        }
        let (retry, start) = downloads.join(key.clone());
        assert!(start);
        downloads.complete(&key, Ok(content()));
        retry.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn whole_book_waiters_select_different_chapters_from_the_same_result() {
        let downloads = Downloads::default();
        let key = CacheKey::BookFile("file".into());
        let (first, start) = downloads.join(key.clone());
        assert!(start);
        let (second, start) = downloads.join(key.clone());
        assert!(!start);
        let chapters: Arc<[Chapter]> = vec![
            Chapter {
                idx: 0,
                title: "第一章".into(),
                content: "<p>甲</p>".into(),
            },
            Chapter {
                idx: 1,
                title: "第二章".into(),
                content: "<p>乙</p>".into(),
            },
        ]
        .into();
        downloads.complete(&key, Ok(DownloadedContent::Book(chapters.clone())));
        let first = first.await.unwrap().unwrap();
        let second = second.await.unwrap().unwrap();
        assert!(matches!(&first,DownloadedContent::Book(book) if Arc::ptr_eq(book,&chapters)));
        assert!(matches!(&second,DownloadedContent::Book(book) if Arc::ptr_eq(book,&chapters)));
        assert_eq!(first.clone().chapter(0).unwrap().content, "<p>甲</p>");
        assert_eq!(second.chapter(1).unwrap().content, "<p>乙</p>");
        assert_eq!(first.chapter(2).unwrap_err(), DownloadError::NotFound);
    }
}
