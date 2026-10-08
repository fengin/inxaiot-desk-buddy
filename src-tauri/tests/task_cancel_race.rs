#[path = "common/screen_test_support.rs"] mod support;
use inxaiot_desk_buddy_lib::{
    core::error::AppError,
    domain::common::task::TaskState,
    infrastructure::local_sqlite::task_repository::CreateTask,
    interface::commands::task_activity::request_task_cancel,
    runtime::task_queue::TaskEnvelope,
};
use std::time::Duration;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn immediate_cancel_survives_queue_dispatch_and_completion_races() {
    let temp=tempfile::tempdir().unwrap();let state=support::state_at(temp.path(),false).await;
    sqlx::query("INSERT INTO local_project(id,name,platform_url,db_host,db_port,db_user,business_db,workbench_db,db_password_secret_ref,created_at,updated_at) VALUES('cancel-race','取消竞争','','',3306,'','','','test','1','1')").execute(state.local_store.pool()).await.unwrap();
    let repository=state.task_repository.clone();
    state.task_handler_registry.register("smart_screen","inspect",move |envelope,cancel|{
        let repository=repository.clone();async move {
            let id=&envelope.local_task_id;
            if repository.transition(id,TaskState::Queued,TaskState::Running,None,None).await.is_err(){return Err(AppError::Cancelled);}
            cancel.cancelled().await;
            loop {
                let current=repository.get(id).await?.state;
                let next=match current {TaskState::Running=>TaskState::Cancelling,TaskState::Cancelling=>TaskState::Cancelled,_=>break};
                if let Err(error)=repository.transition(id,current,next,None,None).await {
                    if !matches!(error,AppError::Conflict(_)){return Err(error);}
                }
            }
            Err(AppError::Cancelled)
        }
    }).unwrap();
    for index in 0..80 {
        let id=format!("cancel-race-{index}");
        state.task_repository.create(CreateTask {id:id.clone(),local_project_id:"cancel-race".into(),remote_operation_record_id:None,domain_type:"smart_screen".into(),operation_type:"inspect".into(),name:"取消竞争".into(),priority:0,batch_size:1,concurrency:1,payload_ref:None,log_path:temp.path().join(format!("{id}.jsonl")).to_string_lossy().into_owned(),targets:vec![("smart_screen".into(),id.clone())]}).await.unwrap();
        for (from,to) in [(TaskState::Draft,TaskState::Checking),(TaskState::Checking,TaskState::Ready),(TaskState::Ready,TaskState::Queued)]{state.task_repository.transition(&id,from,to,None,None).await.unwrap();}
        state.task_queue.enqueue(TaskEnvelope {local_task_id:id.clone(),local_project_id:"cancel-race".into(),domain_type:"smart_screen".into(),operation_type:"inspect".into(),resource_keys:vec![format!("smart_screen:{id}")],priority:0,payload_ref:None,payload_sha256:None}).await.unwrap();
        if index%2==0{tokio::time::sleep(Duration::from_millis(1)).await;}
        let cancelled=request_task_cancel(&state,&id).await;
        assert!(cancelled.is_ok(),"第{index}次立即取消失败：{:?}",cancelled.err());
        tokio::time::timeout(Duration::from_secs(3),async {while !state.task_repository.get(&id).await.unwrap().state.is_terminal(){tokio::time::sleep(Duration::from_millis(1)).await;}}).await.unwrap();
        assert_eq!(state.task_repository.get(&id).await.unwrap().state,TaskState::Cancelled);
    }
    support::close(state).await;
}
