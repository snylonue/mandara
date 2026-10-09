"""Compare every supported spine document's retained text against offline audit exports.

Usage: python3 scripts/check-epub-corpus.py target/epub-audit/rendered
Original EPUBs must be in the export directory's parent. Never contacts a server
or prints publication content; only document counts and hashes are reported.
"""
import hashlib
import json
import re
import sys
import zipfile
from html.parser import HTMLParser
from pathlib import Path
from urllib.parse import unquote, urljoin, urlsplit
from xml.etree import ElementTree as ET


class Text(HTMLParser):
    def __init__(self):
        super().__init__(convert_charrefs=True)
        self.hidden = []
        self.text = []

    def handle_starttag(self, tag, attrs):
        if tag in {"head", "script", "style"}:
            self.hidden.append(tag)

    def handle_endtag(self, tag):
        if self.hidden and tag == self.hidden[-1]:
            self.hidden.pop()

    def handle_data(self, data):
        if not self.hidden:
            self.text.append(data)

    def normalized(self, source):
        self.feed(source)
        return re.sub(r"\s+", "", "".join(self.text))


def decode(raw):
    if raw.startswith((b"\xff\xfe", b"\xfe\xff")):
        return raw.decode("utf-16")
    return raw.decode("utf-8-sig")


def check(snapshot):
    book = json.loads(snapshot.read_text())
    original = snapshot.parent.parent / (snapshot.stem + ".epub")
    with zipfile.ZipFile(original) as archive:
        container = ET.fromstring(archive.read("META-INF/container.xml"))
        package = next(n.attrib["full-path"] for n in container.iter() if n.tag.endswith("rootfile"))
        root = ET.fromstring(archive.read(package))
        ns = {"opf": "http://www.idpf.org/2007/opf"}
        manifest = {n.attrib["id"]: n.attrib for n in root.findall("opf:manifest/opf:item", ns)}
        documents = []
        for entry in root.findall("opf:spine/opf:itemref", ns):
            item = manifest[entry.attrib["idref"]]
            visited = set()
            while item["media-type"] not in {"application/xhtml+xml", "text/html", "image/svg+xml"}:
                if item["id"] in visited or "fallback" not in item:
                    raise AssertionError("Unsupported spine without usable fallback")
                visited.add(item["id"])
                item = manifest[item["fallback"]]
            target = unquote(urlsplit(urljoin("https://epub.invalid/" + package, item["href"])).path).lstrip("/")
            documents.append((entry.attrib.get("linear", "yes") != "no", target))
        documents.sort(key=lambda d: not d[0])
        assert len(book["chapters"]) >= len(documents), snapshot.stem + ": missing spine documents"
        for index, (linear, target) in enumerate(documents):
            chapter = book["chapters"][index]
            assert chapter.get("linear", True) == linear, snapshot.stem + ": changed linear order"
            expected = Text().normalized(decode(archive.read(target)))
            actual = Text().normalized(chapter["content"])
            if expected != actual:
                raise AssertionError(f"{snapshot.stem}: text changed at spine document {index}; source={hashlib.sha256(expected.encode()).hexdigest()} parsed={hashlib.sha256(actual.encode()).hexdigest()}")
        print(f"PASS: {snapshot.stem} all-spine-text={len(documents)} primary={sum(d[0] for d in documents)} auxiliary={sum(not d[0] for d in documents)}")


if __name__ == "__main__":
    exports = sorted(Path(sys.argv[1]).glob("*.json"))
    assert exports, "No EPUB audit snapshots"
    for export in exports:
        check(export)
