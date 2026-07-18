from core.models import ConversationState
from core.conversation_state import StateTracker
from tests.fixtures.sample_segment import make_segment


def test_tracker_extracts_open_question_from_client():
    tracker = StateTracker()
    seg = make_segment(text="What is your MOQ for this part?", language="en", speaker="client")
    state = tracker.update(seg)
    assert any("MOQ" in q or "moq" in q.lower() or "?" in q for q in state.open_questions)
    assert state.open_questions


def test_tracker_updates_topic_from_client_speech():
    tracker = StateTracker()
    seg = make_segment(text="Let's discuss the NPI timeline for Q3", language="en", speaker="client")
    state = tracker.update(seg)
    assert state.topic
    assert "NPI" in state.topic or "timeline" in state.topic.lower() or "Q3" in state.topic


def test_tracker_detects_decision_phrases():
    tracker = StateTracker()
    seg = make_segment(
        text="We agreed to proceed with the sample build next week",
        language="en",
        speaker="client",
    )
    state = tracker.update(seg)
    assert state.recent_decisions
    assert any("agreed" in d.lower() or "sample" in d.lower() for d in state.recent_decisions)


def test_tracker_ignores_user_for_questions():
    tracker = StateTracker()
    seg = make_segment(text="你们的交期是多久？", language="zh", speaker="user")
    state = tracker.update(seg)
    assert state.open_questions == []


def test_conversation_state_as_brief():
    state = ConversationState(
        topic="NPI schedule",
        open_questions=["What is MOQ?"],
        recent_decisions=["Agreed on sample build"],
    )
    text = state.as_brief()
    assert "NPI schedule" in text
    assert "MOQ" in text
    assert "sample" in text
