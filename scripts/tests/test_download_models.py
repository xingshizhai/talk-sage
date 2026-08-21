import importlib.util
from pathlib import Path
import struct
import tempfile
import unittest


SCRIPT = Path(__file__).resolve().parents[1] / "download_models.py"
SPEC = importlib.util.spec_from_file_location("talksage_download_models", SCRIPT)
download_models = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(download_models)


def varint(value: int) -> bytes:
    out = bytearray()
    while value >= 0x80:
        out.append((value & 0x7F) | 0x80)
        value >>= 7
    out.append(value)
    return bytes(out)


def piece(name: str, score: float) -> bytes:
    encoded = name.encode()
    message = b"\x0a" + varint(len(encoded)) + encoded + b"\x15" + struct.pack("<f", score)
    return b"\x0a" + varint(len(message)) + message


class SentencePieceExportTests(unittest.TestCase):
    def test_exports_piece_and_training_score_without_protobuf_dependency(self):
        with tempfile.TemporaryDirectory() as directory:
            model = Path(directory) / "bpe.model"
            vocab = Path(directory) / "bpe.vocab"
            model.write_bytes(piece("▁TALK", -1.25) + piece("SAGE", -2.5))
            download_models.export_sentencepiece_vocab(model, vocab)
            lines = vocab.read_text().splitlines()
            self.assertEqual(lines[0], "▁TALK\t-1.25")
            self.assertEqual(lines[1], "SAGE\t-2.5")


if __name__ == "__main__":
    unittest.main()
