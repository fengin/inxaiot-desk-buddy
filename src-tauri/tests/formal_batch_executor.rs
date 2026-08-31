use std::collections::BTreeMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use inxaiot_desk_buddy_lib::core::error::AppError;
use inxaiot_desk_buddy_lib::runtime::batch_executor::{TargetExecutionOutcome, execute_in_batches};
use tokio_util::sync::CancellationToken;

#[tokio::test]
async fn batches_wait_for_previous_wave_and_limit_target_concurrency() {
    let active = Arc::new(AtomicUsize::new(0));
    let maximum = Arc::new(AtomicUsize::new(0));
    let timings = Arc::new(Mutex::new(BTreeMap::<usize, (Instant, Instant)>::new()));
    let results = execute_in_batches(vec![0_usize, 1, 2, 3, 4], 2, 2, CancellationToken::new(), {
        let active = active.clone();
        let maximum = maximum.clone();
        let timings = timings.clone();
        move |index, _| {
            let active = active.clone();
            let maximum = maximum.clone();
            let timings = timings.clone();
            async move {
                let started = Instant::now();
                let now_active = active.fetch_add(1, Ordering::SeqCst) + 1;
                maximum.fetch_max(now_active, Ordering::SeqCst);
                tokio::time::sleep(Duration::from_millis(30)).await;
                active.fetch_sub(1, Ordering::SeqCst);
                timings
                    .lock()
                    .expect("timings")
                    .insert(index, (started, Instant::now()));
                if index == 3 {
                    Err(AppError::InvalidConfig("isolated target failure".into()))
                } else {
                    Ok(())
                }
            }
        }
    })
    .await
    .expect("execute batches");
    assert!(maximum.load(Ordering::SeqCst) <= 2);
    let timings = timings.lock().expect("timings");
    let first_batch_end = timings[&0].1.max(timings[&1].1);
    assert!(timings[&2].0 >= first_batch_end);
    let second_batch_end = timings[&2].1.max(timings[&3].1);
    assert!(timings[&4].0 >= second_batch_end);
    assert_eq!(results.len(), 5);
    assert!(matches!(
        results[3].outcome,
        TargetExecutionOutcome::Failed(_)
    ));
    assert!(matches!(
        results[4].outcome,
        TargetExecutionOutcome::Completed
    ));
}

#[tokio::test]
async fn cancellation_stops_dispatching_new_batches() {
    let cancellation = CancellationToken::new();
    let started = Arc::new(AtomicUsize::new(0));
    let trigger = cancellation.clone();
    let results = execute_in_batches(vec![0_usize, 1, 2, 3, 4, 5], 2, 2, cancellation, {
        let started = started.clone();
        move |index, cancellation| {
            let started = started.clone();
            let trigger = trigger.clone();
            async move {
                started.fetch_add(1, Ordering::SeqCst);
                if index == 0 {
                    trigger.cancel();
                }
                cancellation.cancelled().await;
                Err(AppError::Cancelled)
            }
        }
    })
    .await
    .expect("cancelled batches");
    assert!(started.load(Ordering::SeqCst) <= 2);
    assert!(
        results
            .iter()
            .all(|result| result.outcome == TargetExecutionOutcome::Cancelled)
    );
}
