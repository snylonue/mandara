#!/usr/bin/env python3
"""Mock remote book source for the demo plugins (docs/plugin-http-api-design.md).

Serves the same book universe the plugins used to hardcode, over plain
HTTP — the plugins now fetch everything through the host's `http.fetch`
import. Two namespaces, one server:

  /wiki/...    metadata-only source: w-N entries pointing
               content-source at the `reader` instance (R5 demo)
  /reader/...  content source: r-N entries with chapter titles/bodies
               and whole epub downloads (file mode)

Endpoints (both namespaces):
  GET  {ns}/api/search?q=&page_size=&offset=&limit=  -> {"total","items":[book-entry]}
  GET  {ns}/api/books/{id}                            -> book-entry | 404
  GET  {ns}/api/books/{id}/chapters                   -> {"titles":[...]}
       (reader only, honors ?max=)
  GET  {ns}/api/books/{id}/chapters/{index}           -> {"title","content"} | 404
  GET  {ns}/api/books/{id}/download                   -> epub bytes (reader only)
  GET  /slow?ms=                                      -> answers after ms (timeout demo)
  GET  /big?bytes=N                                   -> N bytes (size-cap demo)

Usage: python3 scripts/mock-source.py [port]     (default 8765)
"""

import json
import sys
import time
import urllib.parse
import zipfile
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from io import BytesIO

PORT = int(sys.argv[1]) if len(sys.argv) > 1 else 8765

# ---------------------------------------------------------------------------
# book universe (same as the old hardcoded plugin catalogs)
# ---------------------------------------------------------------------------

WIKI_BOOKS = [
    ("w-1", "星海拾遗", ["洛离"], "宇宙边缘电台的周播栏目档案。", "r-1"),
    ("w-2", "雾都侦探手记", ["白川"], "终年有雾的城市里，一间小事务所的接案记录。", "r-2"),
    ("w-3", "剑与茶室", ["山岚"], "隐于山道的茶室，白天泡茶，晚上磨剑。", "r-3"),
    ("w-4", "云端咖啡馆", ["苏晚晴"], "开在平流层边缘的咖啡馆。", "r-4"),
    ("w-5", "旧书店的猫", ["林默"], "每本旧书里都有一枚猫爪印。", "r-5"),
    ("w-6", "时间旅人的信", ["迟舟"], "寄信人来自明天。", "r-6"),
    ("w-7", "深海广播", ["韩潮"], "马里亚纳海沟下的电台信号。", "r-7"),
    ("w-8", "第七封印物语", ["陆离"], "第七道封印之后，世界安静得不像话。", "r-8"),
]

READER_BOOKS = [
    ("r-1", "星海拾遗", ["洛离"], "宇宙边缘电台的周播栏目档案。", ["第一章 回声信标", "第二章 潮汐图书馆", "第三章 末班星舟"]),
    ("r-2", "雾都侦探手记", ["白川"], "终年有雾的城市里，一间小事务所的接案记录。", ["第一章 雾中来信", "第二章 九号站台", "第三章 钟楼谜题"]),
    ("r-3", "剑与茶室", ["山岚"], "隐于山道的茶室，白天泡茶，晚上磨剑。", ["第一章 刀鞘与茶匙", "第二章 双刀店主"]),
    ("r-4", "云端咖啡馆", ["苏晚晴"], "开在平流层边缘的咖啡馆。", ["第一章 海拔三千米的拿铁", "第二章 云上菜单"]),
    ("r-5", "旧书店的猫", ["林默"], "每本旧书里都有一枚猫爪印。", ["第一章 扉页爪印", "第二章 借阅卡背面的名字", "第三章 打烊后的书梯"]),
    ("r-6", "时间旅人的信", ["迟舟"], "寄信人来自明天。", ["第一章 明天寄来的明信片", "第二章 错序的邮票", "第三章 末班邮箱"]),
    ("r-7", "深海广播", ["韩潮"], "马里亚纳海沟下的电台信号。", ["第一章 波长 31.4", "第二章 鲸歌应答"]),
    ("r-8", "第七封印物语", ["陆离"], "第七道封印之后，世界安静得不像话。", ["第一章 石门的刻痕", "第二章 无名的守印人", "第三章 封印之下"]),
]


def chapter_text(book_id: str, title: str) -> str:
    return (
        f"（正文由远程模拟源提供）\n\n{title}。\n\n"
        f"这一章的内容由 mock-source.py 生成：远端数据源把章节文本交给插件，"
        f"插件通过 http.fetch 取得后原样返回给宿主。\n\n"
        f"—— 模拟源 books/{book_id} 服务端正文\n"
    )


def wiki_entry(book, site: str):
    wid, title, authors, desc, reader_id = book
    return {
        "id": wid,
        "title": title,
        "authors": authors,
        "description": f"{desc}（词条由 {site} 提供，全文来自阅读源）",
        "cover_url": None,
        "content_source": "reader",
        "content_id": reader_id,
    }


def reader_entry(book):
    rid, title, authors, desc, _chapters = book
    return {
        "id": rid,
        "title": title,
        "authors": authors,
        "description": f"{desc}（阅读源：远程模拟源）",
        "cover_url": None,
        "content_source": None,
        "content_id": None,
    }


# ---------------------------------------------------------------------------
# epub generation (file-mode downloads)
# ---------------------------------------------------------------------------

EPUBS = {}


def build_epub(book) -> bytes:
    """A real EPUB3: nested nav TOC (卷 -> chapters), sanitizable XHTML."""
    rid, title, _authors, _desc, chapters = book
    buf = BytesIO()
    with zipfile.ZipFile(buf, "w", zipfile.ZIP_DEFLATED) as z:
        z.writestr("mimetype", "application/epub+zip", compress_type=zipfile.ZIP_STORED)
        z.writestr(
            "META-INF/container.xml",
            '<?xml version="1.0"?>\n'
            '<container version="1.0" xmlns="urn:oasis:names:tc:opendocument:xmlns:container">\n'
            '  <rootfiles><rootfile full-path="OEBPS/content.opf" media-type="application/oebps-package+xml"/></rootfiles>\n'
            "</container>\n",
        )
        manifest = ""
        spine = ""
        for i in range(len(chapters)):
            manifest += f'<item id="c{i}" href="ch{i}.xhtml" media-type="application/xhtml+xml"/>'
            spine += f'<itemref idref="c{i}"/>'
        manifest += '<item id="nav" href="nav.xhtml" media-type="application/xhtml+xml" properties="nav"/>'
        z.writestr(
            "OEBPS/content.opf",
            f'<?xml version="1.0"?>\n'
            f'<package xmlns="http://www.idpf.org/2007/opf" version="3.0" unique-identifier="uid">\n'
            f'  <metadata xmlns:dc="http://purl.org/dc/elements/1.1/">\n'
            f'    <dc:identifier id="uid">{rid}</dc:identifier>\n'
            f'    <dc:title>{title}</dc:title>\n'
            f'  </metadata>\n'
            f'  <manifest>{manifest}</manifest>\n'
            f'  <spine>{spine}</spine>\n'
            f"</package>\n",
        )
        # nested TOC: 卷一 owns the first two chapters, 卷二 the rest (or
        # a single 正文 group for 2-chapter books) — proves hierarchical
        # TOC round-trips through the host parser.
        def vol_li(label, items):
            lis = "".join(
                f'<li><a href="ch{i}.xhtml">{ch}</a></li>' for i, ch in items
            )
            return f'<li><span>{label}</span><ol>{lis}</ol></li>'

        vols = [vol_li("卷一", list(enumerate(chapters[:2])))]
        if len(chapters) > 2:
            vols.append(vol_li("卷二", list(enumerate(chapters[2:], start=2))))
        z.writestr(
            "OEBPS/nav.xhtml",
            '<?xml version="1.0"?>\n'
            '<html xmlns="http://www.w3.org/1999/xhtml" xmlns:epub="http://www.idpf.org/2007/ops">\n'
            "<head><title>目录</title></head>\n"
            f'<body><nav epub:type="toc"><ol>{"".join(vols)}</ol></nav></body>\n'
            "</html>\n",
        )
        for i, ch in enumerate(chapters):
            z.writestr(
                f"OEBPS/ch{i}.xhtml",
                '<?xml version="1.0"?>\n'
                '<html xmlns="http://www.w3.org/1999/xhtml">\n'
                f"<head><title>{ch}</title></head>\n"
                "<body>\n"
                f"<h2>{ch}</h2>\n"
                f"<p>（这是 mock-source.py 生成的 epub 正文，第 {i + 1} 节。）</p>\n"
                f"<p>句子以句号结尾。<ruby>書架<rt>bookshelf</rt></ruby>也可以有注音。</p>\n"
                "<blockquote>寄件人来自明天。 —— 模拟正文</blockquote>\n"
                "</body>\n"
                "</html>\n",
            )
    return buf.getvalue()


for book in READER_BOOKS:
    EPUBS[book[0]] = build_epub(book)


# ---------------------------------------------------------------------------
# handler
# ---------------------------------------------------------------------------


def json_response(body: dict, status=200):
    data = json.dumps(body, ensure_ascii=False).encode("utf-8")
    return (status, "application/json; charset=utf-8", data)


class Handler(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def log_message(self, fmt, *args):  # one line per request, for e2e debugging
        print(f"[mock] {self.address_string()} {fmt % args}", flush=True)

    # -- helpers ------------------------------------------------------------

    def send(self, status, content_type, body: bytes):
        self.send_response(status)
        self.send_header("Content-Type", content_type)
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        if self.command != "HEAD":
            self.wfile.write(body)

    def json(self, obj, status=200):
        status, ctype, body = json_response(obj, status)
        self.send(status, ctype, body)

    # -- routing -------------------------------------------------------------

    def do_GET(self):
        parsed = urllib.parse.urlparse(self.path)
        path = parsed.path
        query = urllib.parse.parse_qs(parsed.query)

        if path == "/slow":
            ms = int(query.get("ms", ["1000"])[0])
            time.sleep(ms / 1000.0)
            self.send(200, "text/plain", b"finally")
            return
        if path == "/big":
            n = int(query.get("bytes", ["1024"])[0])
            self.send(200, "application/octet-stream", b"x" * n)
            return

        parts = path.split("/", 3)
        ns = parts[1] if len(parts) > 1 else ""
        rest = ("/" + "/".join(parts[2:])) if len(parts) > 2 else ""
        if ns not in ("wiki", "reader") or not rest.startswith("/api/"):
            self.send(404, "text/plain", b"not found")
            return
        books = WIKI_BOOKS if ns == "wiki" else READER_BOOKS
        self.route_namespace(ns, books, rest, query)

    def route_namespace(self, ns, books, rest, query):
        if rest == "/api/search":
            q = query.get("q", [""])[0].lower()
            page_size = int(query.get("page_size", ["8"])[0])
            offset = int(query.get("offset", ["0"])[0])
            limit = int(query.get("limit", ["50"])[0])
            site = "维基书源" if ns == "wiki" else "远程模拟源"
            all_items = [
                wiki_entry(b, site) if ns == "wiki" else reader_entry(b)
                for b in books[:page_size]
                if not q
                or q in b[1].lower()
                or q in b[0].lower()
                or any(q in a.lower() for a in b[2])
            ]
            self.json({"total": len(all_items), "items": all_items[offset : offset + limit]})
            return

        # /api/books/{id}[/chapters[/{index}]|/download]
        parts = rest.split("/")
        # parts == ["", "api", "books", id, ...]
        if len(parts) < 4:
            self.send(404, "text/plain", b"not found")
            return
        book_id = parts[3]
        book = next((b for b in books if b[0] == book_id), None)
        action = parts[4] if len(parts) > 4 else None

        if action is None:
            if book is None:
                self.send(404, "text/plain", b"not found")
                return
            site = "维基书源" if ns == "wiki" else "远程模拟源"
            self.json(wiki_entry(book, site) if ns == "wiki" else reader_entry(book))
            return

        if ns != "reader":
            self.send(404, "text/plain", b"not found")
            return
        if book is None:
            self.send(404, "text/plain", b"not found")
            return

        if action == "chapters" and len(parts) == 5:
            max_ch = int(query.get("max", ["10"])[0])
            titles = book[4][:max_ch]
            self.json({"titles": titles})
            return
        if action == "chapters" and len(parts) == 6:
            try:
                index = int(parts[5])
            except ValueError:
                self.send(404, "text/plain", b"not found")
                return
            if index >= len(book[4]):
                self.send(404, "text/plain", b"not found")
                return
            self.json({"title": book[4][index], "content": chapter_text(book[0], book[4][index])})
            return
        if action == "download":
            self.send(
                200,
                "application/epub+zip",
                EPUBS[book[0]],
            )
            return
        self.send(404, "text/plain", b"not found")


if __name__ == "__main__":
    server = ThreadingHTTPServer(("127.0.0.1", PORT), Handler)
    print(f"mock source listening on http://127.0.0.1:{PORT}", flush=True)
    server.serve_forever()