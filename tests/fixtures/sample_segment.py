from core.models import TranscriptSegment


def make_segment(
    text: str = "our NPI schedule starts in Q3",
    speaker: str = "client",
    language: str = "en",
) -> TranscriptSegment:
    return TranscriptSegment(speaker=speaker, text=text, language=language, timestamp=1000.0)
