//! Transport-agnostic event sink for pipeline::execute.
//!
//! PipelineEvent = StreamEvent (web SSE events) | ProcessingPhase (channel typing).
//! Each sink chooses which variants to forward and silently drops the rest.

use crate::agent::engine::stream::ProcessingPhase;
use crate::agent::stream_event::StreamEvent;

#[derive(Debug, Clone)]
pub enum PipelineEvent {
    Stream(StreamEvent),
    Phase(ProcessingPhase),
}

impl From<StreamEvent> for PipelineEvent {
    fn from(ev: StreamEvent) -> Self {
        PipelineEvent::Stream(ev)
    }
}
impl From<ProcessingPhase> for PipelineEvent {
    fn from(p: ProcessingPhase) -> Self {
        PipelineEvent::Phase(p)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum SinkError {
    #[error("sink closed (client disconnected)")]
    Closed,
    #[error("sink full (backpressure)")]
    Full,
    #[error(transparent)]
    Fatal(#[from] anyhow::Error),
}

pub trait EventSink: Send {
    async fn emit(&mut self, ev: PipelineEvent) -> Result<(), SinkError>;
    async fn close(&mut self) -> Result<(), SinkError> {
        Ok(())
    }
}

#[cfg(test)]
pub mod test_support {
    use super::*;

    #[derive(Default, Debug)]
    pub struct MockSink {
        pub events: Vec<PipelineEvent>,
        pub closed_after: Option<usize>,
    }

    impl MockSink {
        pub fn new() -> Self {
            Self::default()
        }
        pub fn close_after(n: usize) -> Self {
            Self { closed_after: Some(n), ..Self::default() }
        }

        pub fn stream_shapes(&self) -> Vec<&'static str> {
            self.events
                .iter()
                .filter_map(|e| match e {
                    PipelineEvent::Stream(StreamEvent::MessageStart { .. }) => Some("MessageStart"),
                    PipelineEvent::Stream(StreamEvent::TextDelta(_)) => Some("TextDelta"),
                    PipelineEvent::Stream(StreamEvent::Finish { .. }) => Some("Finish"),
                    PipelineEvent::Stream(StreamEvent::Error(_)) => Some("Error"),
                    PipelineEvent::Stream(StreamEvent::ToolCallStart { .. }) => Some("ToolCallStart"),
                    PipelineEvent::Stream(StreamEvent::ToolResult { .. }) => Some("ToolResult"),
                    _ => None,
                })
                .collect()
        }
    }

    impl EventSink for MockSink {
        async fn emit(&mut self, ev: PipelineEvent) -> Result<(), SinkError> {
            if let Some(n) = self.closed_after {
                if self.events.len() >= n {
                    return Err(SinkError::Closed);
                }
            }
            self.events.push(ev);
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::test_support::MockSink;

    #[tokio::test]
    async fn mock_sink_records_events() {
        let mut sink = MockSink::new();
        sink.emit(StreamEvent::TextDelta("a".into()).into()).await.unwrap();
        sink.emit(ProcessingPhase::Thinking.into()).await.unwrap();
        assert_eq!(sink.events.len(), 2);
    }

    #[tokio::test]
    async fn mock_sink_closes_after_limit() {
        let mut sink = MockSink::close_after(1);
        sink.emit(StreamEvent::TextDelta("ok".into()).into()).await.unwrap();
        let err = sink.emit(StreamEvent::TextDelta("drop".into()).into()).await;
        assert!(matches!(err, Err(SinkError::Closed)));
    }
}
