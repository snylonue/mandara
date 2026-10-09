//! Read-only EPUB diagnostics. Optional exports stay outside the production DB.
use base64::{Engine, engine::general_purpose::STANDARD};
fn main() {
    let output = std::env::var_os("MANDARA_EPUB_AUDIT_OUTPUT_DIR").map(std::path::PathBuf::from);
    for path in std::env::args().skip(1) {
        let bytes = std::fs::read(&path).expect("read EPUB");
        match mandara_formats::epub::parse_with_diagnostics(&bytes) {
            Ok((book, diagnostics)) => {
                println!(
                    "{path}: chapters={} images={} toc={} authors={}",
                    book.chapters.len(),
                    book.images.len(),
                    book.toc.len(),
                    book.authors.len()
                );
                let mut counts = std::collections::BTreeMap::new();
                for issue in &diagnostics {
                    *counts.entry(issue.reason).or_insert(0) += 1;
                }
                println!("  diagnostics={counts:?}");
                if let Some(output) = &output {
                    std::fs::create_dir_all(output).expect("create audit output");
                    let chapters: Vec<_> = book.chapters.iter().enumerate().map(|(idx, chapter)| {
                        let mut content = chapter.content.clone();
                        for (n, image) in book.images.iter().enumerate() {
                            content = content.replace(&format!("src=\"image:{n}\""), &format!("src=\"data:{};base64,{}\"", image.mime, STANDARD.encode(&image.bytes)));
                        }
                        serde_json::json!({ "idx": idx, "title": chapter.title, "content": content, "linear": chapter.linear })
                    }).collect();
                    let issues: Vec<_> = diagnostics.iter().map(|d| serde_json::json!({ "document": d.document, "reference": d.reference, "reason": d.reason })).collect();
                    let name = std::path::Path::new(&path).file_stem().expect("file stem");
                    let target = output.join(name).with_extension("json");
                    let json = serde_json::json!({ "title": book.title, "chapters": chapters, "toc": book.toc, "diagnostics": issues });
                    std::fs::write(target, serde_json::to_vec(&json).unwrap())
                        .expect("export audit snapshot");
                }
            }
            Err(error) => println!("{path}: ERROR {error}"),
        }
    }
}
