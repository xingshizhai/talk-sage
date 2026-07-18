from core.echo_filter import CrosstalkFilter
from core.models import TranscriptSegment
import time


def test_drops_user_echo_of_recent_client():
    filt = CrosstalkFilter(similarity_threshold=0.6, window_seconds=10)
    client = TranscriptSegment(speaker="client", text="our NPI schedule starts in Q3", language="en")
    filt.observe(client)
    user = TranscriptSegment(speaker="user", text="our NPI schedule starts in Q3", language="zh")
    assert filt.should_drop(user) is True


def test_keeps_distinct_user_speech():
    filt = CrosstalkFilter(similarity_threshold=0.6, window_seconds=10)
    client = TranscriptSegment(speaker="client", text="our NPI schedule starts in Q3", language="en")
    filt.observe(client)
    user = TranscriptSegment(speaker="user", text="我们下周安排打样", language="zh")
    assert filt.should_drop(user) is False


def test_keeps_when_outside_time_window():
    filt = CrosstalkFilter(similarity_threshold=0.6, window_seconds=1)
    client = TranscriptSegment(
        speaker="client",
        text="please review the BOQ carefully",
        language="en",
        timestamp=time.time() - 30,
    )
    filt.observe(client)
    user = TranscriptSegment(
        speaker="user",
        text="please review the BOQ carefully",
        language="zh",
    )
    assert filt.should_drop(user) is False


def test_jaccard_similarity_basic():
    from core.echo_filter import text_similarity
    assert text_similarity("hello world", "hello world") == 1.0
    assert text_similarity("hello world", "goodbye") < 0.5
