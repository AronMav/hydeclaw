use std::sync::Arc;
use tokio::sync::OnceCell;
use tokio_util::sync::CancellationToken;

use super::error::CancelReason;

/// Single-writer slot for the reason a stream was cancelled.
/// Writers MUST `set()` BEFORE cancelling the token, so readers that
/// wake on `token.cancelled()` always see a populated reason.
#[derive(Debug, Clone)]
pub struct CancelSlot(Arc<OnceCell<CancelReason>>);

impl CancelSlot {
    pub fn new() -> Self {
        Self(Arc::new(OnceCell::new()))
    }

    /// Returns Ok if this writer won the race, Err(losing_reason) if another writer already set the slot.
    pub fn set(&self, reason: CancelReason) -> Result<(), CancelReason> {
        self.0.set(reason).map_err(|_| reason)
    }

    pub fn get(&self) -> Option<CancelReason> {
        self.0.get().copied()
    }
}

impl Default for CancelSlot {
    fn default() -> Self {
        Self::new()
    }
}

/// Convenience: set reason, then cancel. Correct ordering for the
/// "readers always see a populated reason" invariant.
pub fn set_and_cancel(slot: &CancelSlot, token: &CancellationToken, reason: CancelReason) {
    let _ = slot.set(reason);
    token.cancel();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn single_writer_wins() {
        let slot = CancelSlot::new();
        assert!(slot.set(CancelReason::UserCancelled).is_ok());
        assert_eq!(
            slot.set(CancelReason::ShutdownDrain),
            Err(CancelReason::ShutdownDrain)
        );
        assert_eq!(slot.get(), Some(CancelReason::UserCancelled));
    }

    #[tokio::test]
    async fn set_and_cancel_orders_writes() {
        let slot = CancelSlot::new();
        let token = CancellationToken::new();

        let observer_slot = slot.clone();
        let observer_token = token.clone();
        let task = tokio::spawn(async move {
            observer_token.cancelled().await;
            observer_slot.get()
        });

        set_and_cancel(
            &slot,
            &token,
            CancelReason::MaxDurationExceeded { elapsed_secs: 600 },
        );
        let reason = task.await.unwrap();
        assert!(matches!(
            reason,
            Some(CancelReason::MaxDurationExceeded { elapsed_secs: 600 })
        ));
    }

    #[tokio::test]
    async fn race_only_first_wins() {
        let slot = CancelSlot::new();
        let token = CancellationToken::new();

        let handles: Vec<_> = (0..8)
            .map(|i| {
                let s = slot.clone();
                let t = token.clone();
                tokio::spawn(async move {
                    let reason = if i % 2 == 0 {
                        CancelReason::UserCancelled
                    } else {
                        CancelReason::ShutdownDrain
                    };
                    set_and_cancel(&s, &t, reason);
                })
            })
            .collect();

        for h in handles {
            h.await.unwrap();
        }
        assert!(slot.get().is_some());
    }
}
