from __future__ import annotations

import re
from dataclasses import dataclass
from pathlib import Path

_TOKEN_RE = re.compile(r"[A-Za-z0-9\u4e00-\u9fff]{2,}")
_HEADING_RE = re.compile(r"^#{1,6}\s+(.+)$", re.M)


@dataclass
class KBChunk:
    text: str
    source: str
    heading: str = ""
    score: float = 0.0


class KnowledgeBase:
    """Local folder knowledge base with keyword Jaccard retrieval (no embeddings)."""

    def __init__(self):
        self._chunks: list[KBChunk] = []

    @property
    def chunk_count(self) -> int:
        return len(self._chunks)

    def index_folder(self, folder: Path | str) -> int:
        path = Path(folder)
        self._chunks = []
        if not path.is_dir():
            return 0
        for file in sorted(path.rglob("*")):
            if file.suffix.lower() not in {".md", ".txt"}:
                continue
            if not file.is_file():
                continue
            try:
                text = file.read_text(encoding="utf-8")
            except OSError:
                continue
            rel = str(file.relative_to(path))
            self._chunks.extend(self._chunk_file(text, rel))
        return len(self._chunks)

    def search(self, query: str, top_k: int = 3, min_score: float = 0.05) -> list[KBChunk]:
        if not self._chunks or not query.strip():
            return []
        q_tokens = set(_TOKEN_RE.findall(query.lower()))
        if not q_tokens:
            return []
        scored: list[KBChunk] = []
        for chunk in self._chunks:
            c_tokens = set(_TOKEN_RE.findall(chunk.text.lower()))
            if not c_tokens:
                continue
            score = len(q_tokens & c_tokens) / len(q_tokens | c_tokens)
            # Boost exact token hits for short queries
            overlap = len(q_tokens & c_tokens)
            if overlap:
                score = max(score, overlap / max(len(q_tokens), 1) * 0.5)
            if score >= min_score:
                scored.append(
                    KBChunk(
                        text=chunk.text,
                        source=chunk.source,
                        heading=chunk.heading,
                        score=score,
                    )
                )
        scored.sort(key=lambda c: c.score, reverse=True)
        return scored[:top_k]

    def _chunk_file(self, text: str, source: str) -> list[KBChunk]:
        parts = re.split(r"(?=^#{1,6}\s+)", text, flags=re.M)
        chunks: list[KBChunk] = []
        for part in parts:
            part = part.strip()
            if not part:
                continue
            heading_match = _HEADING_RE.match(part)
            heading = heading_match.group(1).strip() if heading_match else ""
            # Split oversized sections
            if len(part) > 800:
                paragraphs = [p.strip() for p in part.split("\n\n") if p.strip()]
                buf: list[str] = []
                for p in paragraphs:
                    buf.append(p)
                    if sum(len(x) for x in buf) >= 400:
                        chunks.append(KBChunk(text="\n\n".join(buf), source=source, heading=heading))
                        buf = []
                if buf:
                    chunks.append(KBChunk(text="\n\n".join(buf), source=source, heading=heading))
            else:
                chunks.append(KBChunk(text=part, source=source, heading=heading))
        if not chunks and text.strip():
            chunks.append(KBChunk(text=text.strip(), source=source))
        return chunks
