use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use inxaiot_desk_buddy_lib::runtime::batch_executor::{TargetExecutionOutcome, execute_in_batches};
use tokio_util::sync::CancellationToken;

#[tokio::test]
async fn three_hundred_targets_finish_with_bounded_concurrency_and_stable_order() {
    let active = Arc::new(AtomicUsize::new(0));
    let maximum = Arc::new(AtomicUsize::new(0));
    let started = Instant::now();
    let results = execute_in_batches(
        (0_u16..300).collect::<Vec<_>>(),
        50,
        8,
        CancellationToken::new(),
        {
            let active = active.clone();
            let maximum = maximum.clone();
            move |_target, _cancellation| {
                let active = active.clone();
                let maximum = maximum.clone();
                async move {
                    let current = active.fetch_add(1, Ordering::SeqCst) + 1;
                    maximum.fetch_max(current, Ordering::SeqCst);
                    tokio::time::sleep(Duration::from_millis(2)).await;
                    active.fetch_sub(1, Ordering::SeqCst);
                    Ok(())
                }
            }
        },
    )
    .await
    .expect("execute 300 targets");
    assert_eq!(results.len(), 300);
    assert!(
        results
            .iter()
            .all(|result| result.outcome == TargetExecutionOutcome::Completed)
    );
    assert!(results.windows(2).all(|pair| pair[0].index < pair[1].index));
    assert!(maximum.load(Ordering::SeqCst) <= 8);
    assert!(started.elapsed() < Duration::from_secs(5));
}

#[tokio::test]
async fn cancellation_stops_new_batches_and_finalizes_remaining_targets_as_cancelled() {
    let cancellation = CancellationToken::new();
    let completed = Arc::new(AtomicUsize::new(0));
    let results = execute_in_batches(
        (0_u16..300).collect::<Vec<_>>(),
        20,
        4,
        cancellation.clone(),
        {
            let completed = completed.clone();
            move |_target, token| {
                let cancellation = cancellation.clone();
                let completed = completed.clone();
                async move {
                    let count = completed.fetch_add(1, Ordering::SeqCst) + 1;
                    if count == 4 {
                        cancellation.cancel();
                    }
                    tokio::select! {
                        _ = token.cancelled() => Err(inxaiot_desk_buddy_lib::core::error::AppError::Cancelled),
                        _ = tokio::time::sleep(Duration::from_millis(5)) => Ok(()),
                    }
                }
            }
        },
    )
    .await
    .expect("cancel 300 target execution");
    assert_eq!(results.len(), 300);
    assert!(
        results
            .iter()
            .filter(|result| result.outcome == TargetExecutionOutcome::Cancelled)
            .count()
            >= 296
    );
}
