//! 小新配置读取与预检共用进程级许可；单屏内部不再次取许可，不启动脱管任务。
use futures_util::{StreamExt, stream};
use std::{future::Future, sync::OnceLock};
use tokio::sync::Semaphore;

pub const LIMIT: usize = 3;

fn slots() -> &'static Semaphore {
    static SLOTS: OnceLock<Semaphore> = OnceLock::new();
    SLOTS.get_or_init(|| Semaphore::new(LIMIT))
}

pub(super) async fn ordered<T,R,F,Fut>(inputs:Vec<T>,limit:usize,operation:F)->Vec<R>
where F:Fn(T)->Fut,Fut:Future<Output=R> {
    ordered_with_slots(inputs,limit,operation,slots()).await
}

/// 预检基础设施错误是整批致命错误；立即停派发并取消仍在途的只读工作。
pub(super) async fn try_ordered<T,R,E,F,Fut>(inputs:Vec<T>,limit:usize,operation:F)->Result<Vec<R>,E>
where F:Fn(T)->Fut,Fut:Future<Output=Result<R,E>> {
    try_ordered_with_slots(inputs,limit,operation,slots()).await
}

async fn try_ordered_with_slots<T,R,E,F,Fut>(inputs:Vec<T>,limit:usize,operation:F,semaphore:&Semaphore)->Result<Vec<R>,E>
where F:Fn(T)->Fut,Fut:Future<Output=Result<R,E>> {
    let count=inputs.len();let mut completed=(0..count).map(|_|None).collect::<Vec<Option<R>>>();
    let mut pending=stream::iter(inputs.into_iter().enumerate().map(|(index,input)| {
        let operation=&operation;
        async move {
            let _permit=semaphore.acquire().await.expect("configuration read semaphore remains open");
            operation(input).await.map(|result|(index,result))
        }
    })).buffer_unordered(limit.clamp(1,LIMIT));
    while let Some(result)=pending.next().await {
        let (index,result)=result?;
        completed[index]=Some(result);
    }
    Ok(completed.into_iter().map(|value|value.expect("each input completes once")).collect())
}

async fn ordered_with_slots<T,R,F,Fut>(inputs:Vec<T>,limit:usize,operation:F,semaphore:&Semaphore)->Vec<R>
where F:Fn(T)->Fut,Fut:Future<Output=R> {
    let count=inputs.len();
    let mut completed=(0..count).map(|_|None).collect::<Vec<Option<R>>>();
    let mut pending=stream::iter(inputs.into_iter().enumerate().map(|(index,input)| {
        let operation=&operation;
        async move {
            // 仅本模块持有该Semaphore且不关闭；许可覆盖整段单屏命令/检查。
            let _permit=semaphore.acquire().await.expect("configuration read semaphore remains open");
            (index,operation(input).await)
        }
    })).buffer_unordered(limit.clamp(1,LIMIT));
    while let Some((index,result))=pending.next().await {completed[index]=Some(result)}
    completed.into_iter().map(|value|value.expect("each input completes once")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc,atomic::{AtomicUsize,Ordering}};
    use std::time::Duration;

    async fn measured(id:usize,current:&AtomicUsize,peak:&AtomicUsize)->Result<usize,&'static str> {
        let active=current.fetch_add(1,Ordering::SeqCst)+1;
        peak.fetch_max(active,Ordering::SeqCst);
        tokio::time::sleep(Duration::from_millis((8-id%7) as u64*2)).await;
        current.fetch_sub(1,Ordering::SeqCst);
        if id==3 {Err("one screen failed")} else {Ok(id)}
    }
    #[tokio::test]
    async fn unordered_completion_preserves_input_positions_and_isolates_screen_errors() {
        let semaphore=Semaphore::new(3);let current=AtomicUsize::new(0);let peak=AtomicUsize::new(0);
        let result=ordered_with_slots((0..9).collect(),3,|id|measured(id,&current,&peak),&semaphore).await;
        assert_eq!(peak.load(Ordering::SeqCst),3);
        assert_eq!(result,(0..9).map(|id|if id==3 {Err("one screen failed")} else {Ok(id)}).collect::<Vec<_>>());
        assert_eq!(semaphore.available_permits(),3);
    }
    #[tokio::test]
    async fn batch_limit_one_remains_serial_and_larger_limits_are_capped() {
        for (limit,expected) in [(1,1),(99,3)] {
            let semaphore=Semaphore::new(3);let current=AtomicUsize::new(0);let peak=AtomicUsize::new(0);
            ordered_with_slots((0..9).collect(),limit,|id|measured(id,&current,&peak),&semaphore).await;
            assert_eq!(peak.load(Ordering::SeqCst),expected);
        }
    }
    #[tokio::test]
    async fn separate_invokes_share_the_same_three_slots_even_when_old_response_is_ignored() {
        let current=Arc::new(AtomicUsize::new(0));let peak=Arc::new(AtomicUsize::new(0));
        let (old_current,old_peak)=(current.clone(),peak.clone());
        let old=tokio::spawn(async move {ordered((0..9).collect(),3,|id|measured(id,&old_current,&old_peak)).await});
        // 前端切换项目而不再接收响应，不代表后端旧调用已经退出。
        tokio::task::yield_now().await;
        let new=ordered((10..19).collect(),3,|id|measured(id,&current,&peak)).await;
        let old=old.await.unwrap();
        assert_eq!(old.len()+new.len(),18);
        assert!(peak.load(Ordering::SeqCst)<=3);
        assert_eq!(current.load(Ordering::SeqCst),0);
    }
    #[tokio::test]
    async fn dropping_a_pending_batch_releases_held_slots_without_spawning_work() {
        let semaphore=Semaphore::new(3);
        let work=ordered_with_slots((0..8).collect(),3,|_|std::future::pending::<()>(),&semaphore);
        assert!(tokio::time::timeout(Duration::from_millis(10),work).await.is_err());
        assert_eq!(semaphore.available_permits(),3);
    }
    #[tokio::test]
    async fn fatal_check_error_stops_new_dispatch_and_drops_inflight_reads() {
        let semaphore=Semaphore::new(3);let started=AtomicUsize::new(0);let finished=AtomicUsize::new(0);
        let (started_ref,finished_ref)=(&started,&finished);
        let result=try_ordered_with_slots((0..20).collect(),3,|id|async move {
            started_ref.fetch_add(1,Ordering::SeqCst);
            if id==1 {tokio::time::sleep(Duration::from_millis(5)).await;return Err("fatal platform read")}
            tokio::time::sleep(Duration::from_secs(1)).await;
            finished_ref.fetch_add(1,Ordering::SeqCst);Ok(id)
        },&semaphore).await;
        assert_eq!(result,Err("fatal platform read"));
        assert_eq!(started.load(Ordering::SeqCst),3);assert_eq!(finished.load(Ordering::SeqCst),0);
        assert_eq!(semaphore.available_permits(),3);
    }
    #[tokio::test]
    async fn ordinary_blocked_screen_is_a_result_and_does_not_cancel_its_neighbors() {
        let semaphore=Semaphore::new(3);
        let result=try_ordered_with_slots((0..9).collect(),3,|id|async move {Ok::<_,&str>((id,if id==1 {"blocked"} else {"ready"}))},&semaphore).await.unwrap();
        assert_eq!(result.len(),9);assert_eq!(result[1],(1,"blocked"));assert_eq!(result[8],(8,"ready"));
    }
}
