//! 回归专用设备命令计数，不记录命令参数、配置内容或凭据。
use inxaiot_desk_buddy_lib::{core::error::AppResult,infrastructure::smart_screen::device::{DeviceCommandPort,ProcessOutput}};
use serde::Serialize;
use std::{collections::BTreeMap,path::Path,sync::{Arc,Mutex,atomic::{AtomicUsize,Ordering}},time::Duration};
use tokio_util::sync::CancellationToken;

#[derive(Default)]
pub struct Counts {active:AtomicUsize,peak:AtomicUsize,total:AtomicUsize,adb:AtomicUsize,by_ip:Mutex<BTreeMap<String,usize>>,
    methods:Mutex<BTreeMap<String,usize>>,methods_by_ip:Mutex<BTreeMap<String,BTreeMap<String,usize>>>}
#[derive(Debug,Serialize)]
#[serde(rename_all="camelCase")]
pub struct CountSnapshot {pub total:usize,pub adb:usize,pub max_concurrent_commands:usize,pub by_ip:BTreeMap<String,usize>,
    pub provider_methods:BTreeMap<String,usize>,pub provider_methods_by_ip:BTreeMap<String,BTreeMap<String,usize>>}
impl Counts {
    pub fn snapshot(&self)->CountSnapshot {CountSnapshot {total:self.total.load(Ordering::SeqCst),adb:self.adb.load(Ordering::SeqCst),
        max_concurrent_commands:self.peak.load(Ordering::SeqCst),by_ip:self.by_ip.lock().unwrap().clone(),
        provider_methods:self.methods.lock().unwrap().clone(),provider_methods_by_ip:self.methods_by_ip.lock().unwrap().clone()}}
}
struct Active(Arc<Counts>);
impl Drop for Active {fn drop(&mut self){self.0.active.fetch_sub(1,Ordering::SeqCst);}}
#[derive(Clone)]
pub struct Counted<P> {pub inner:P,pub counts:Arc<Counts>}
impl<P:DeviceCommandPort> DeviceCommandPort for Counted<P> {
    async fn execute(&self,program:&Path,args:Vec<String>,limit:Duration,cancel:CancellationToken)->AppResult<ProcessOutput> {
        let active=self.counts.active.fetch_add(1,Ordering::SeqCst)+1;let _active=Active(self.counts.clone());
        self.counts.peak.fetch_max(active,Ordering::SeqCst);self.counts.total.fetch_add(1,Ordering::SeqCst);
        if args.first().is_some_and(|arg|arg=="-s" || arg=="connect") {self.counts.adb.fetch_add(1,Ordering::SeqCst);}
        let ip=args.windows(2).find(|p|p[0]=="-s" || p[0]=="connect").map(|p|p[1].split(':').next().unwrap_or("").to_string())
            .or_else(||args.last().filter(|v|v.parse::<std::net::Ipv4Addr>().is_ok()).cloned());
        if let Some(ip)=ip {
            *self.counts.by_ip.lock().unwrap().entry(ip.clone()).or_default()+=1;
            if let Some(method)=args.windows(2).find(|p|p[0]=="--method").map(|p|p[1].clone()) {
                *self.counts.methods.lock().unwrap().entry(method.clone()).or_default()+=1;
                *self.counts.methods_by_ip.lock().unwrap().entry(ip).or_default().entry(method).or_default()+=1;
            }
        }
        self.inner.execute(program,args,limit,cancel).await
    }
}
