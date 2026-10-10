<script setup lang="ts">
import { computed, onBeforeUnmount, ref, watch } from "vue";
import { NAlert,NButton,NModal,NPagination,NTag } from "naive-ui";
import { previewScreenLocks,releaseScreenLocks,type ScreenOperationLock } from "@/shared/api/operationHistory";
import { listBusinessOperationHistory,getBusinessOperationHistoryDetail } from "@/shared/api/operationHistory";
import { commandErrorText } from "@/shared/api/errors";
import { formatDisplayDateTime } from "@/shared/format/dateTime";
import { appConfigFieldLabel,appConfigDisplay } from "@/shared/model/screenAppConfig";
import { ntpServerDisplay, ntpStageLabel } from "@/shared/model/screenNtp";
import type { OperationHistoryPage,OperationHistoryDetail,OperationHistoryTarget } from "@/shared/model/deploymentWorkflow";
import CompactOperationTable from "@/shared/components/CompactOperationTable.vue";
const props=defineProps<{projectId:string}>();
const page=ref(1),loading=ref(false),error=ref(""),detail=ref<OperationHistoryDetail>();
const data=ref<OperationHistoryPage>({items:[],total:0,page:1,pageSize:20});
const targetPage=ref(1),targetPageSize=ref(50);
const targetColumns=computed(()=>[{key:'name',title:'目标屏',width:'18%'},{key:'ip',title:'IP 地址',width:'104px'},{key:'state',title:'结果',width:'84px'},{key:'change',title:detail.value?.operation.operationType==='app_config'?'修改内容':detail.value?.operation.operationType==='ntp'?'NTP 授时':'应用版本',width:'23%'},{key:'message',title:'说明'}]);
const visibleTargets=computed(()=>detail.value?.targets.slice((targetPage.value-1)*targetPageSize.value,targetPage.value*targetPageSize.value)??[]);
const lockColumns=[{key:'resource',title:'占用范围',width:'32%'},{key:'user',title:'操作人',width:'24%'},{key:'computer',title:'来源电脑'}];
watch(()=>detail.value?.operation.id,()=>{targetPage.value=1;});
watch(targetPageSize,()=>{targetPage.value=1;});
function changeSummary(target:OperationHistoryTarget){return target.details?.configuration?.changes?.map(change=>`${appConfigFieldLabel(change.field)}：${appConfigDisplay(change.before,change.field==='environments.current')} → ${appConfigDisplay(change.after,change.field==='environments.current')}`).join('；')||'';}
function configSummary(target:OperationHistoryTarget){const config=target.details?.configuration;return config?`保存：${configLabels[config.save??'pending']} · 重启：${configLabels[config.restart??'pending']} · 回读：${configLabels[config.readback??'pending']}`:'';}
function ntpSummary(target:OperationHistoryTarget){const ntp=target.details?.ntp;return ntp?`保存：${ntpStageLabel(ntp.save)} · 生效：${ntpStageLabel(ntp.activation)} · 授时：${ntpStageLabel(ntp.sync)}`:'';}
function ntpAddressSummary(target:OperationHistoryTarget){const ntp=target.details?.ntp;return ntp?`${ntp.beforeServer == null ? '未读取' : ntpServerDisplay(ntp.beforeServer)} → ${ntp.targetServer == null ? '未记录' : ntpServerDisplay(ntp.targetServer)}`:'未记录';}
function targetSummary(target:OperationHistoryTarget){return [target.resultSummary||target.errorSummary||'—',target.details?`设备：${labels[target.details.device??'not_required']} · 平台数据：${labels[target.details.business??'not_required']}`:'',configSummary(target),ntpSummary(target),target.details?.observedAt?`实测时间：${formatDisplayDateTime(target.details.observedAt)}`:''].filter(Boolean).join('\n');}
const releaseBusy=ref(false),notice=ref("");
const releasePreview=ref<{project:string;operation:string;locks:ScreenOperationLock[]}>();
async function prepareRelease(){
  if(!detail.value)return;
  const project=props.projectId,operation=detail.value.operation.id;releaseBusy.value=true;error.value="";notice.value="";
  try{const locks=await previewScreenLocks(project,operation);if(props.projectId!==project||detail.value?.operation.id!==operation)return;
    if(!locks.length)notice.value="该操作当前没有需要释放的占用";else releasePreview.value={project,operation,locks};
  }catch(cause){error.value=commandErrorText(cause,"读取占用失败");}finally{releaseBusy.value=false;}
}
async function confirmRelease(){
  const preview=releasePreview.value;if(!preview||preview.project!==props.projectId)return;
  releaseBusy.value=true;error.value="";
  try{await releaseScreenLocks(preview.project,preview.operation,preview.locks);releasePreview.value=undefined;
    if(props.projectId===preview.project){notice.value="占用已释放。请先检查屏的实际状态，再发起新操作。";await load();await open(preview.operation);}
  }catch(cause){error.value=commandErrorText(cause,"释放失败，请重新查看占用");releasePreview.value=undefined;}finally{releaseBusy.value=false;}
}
let request=0,detailRequest=0;
const labels:Record<string,string>={running:"处理中",succeeded:"已完成",partially_succeeded:"部分成功",failed:"未完成",cancelled:"已取消",interrupted:"结果待核实",pending:"待保存",unknown:"待核实",skipped:"无需执行",not_required:"无需更新"};
const configLabels:Record<string,string>={pending:'等待处理',unknown:'待核实',saved:'已保存',verified:'已回读确认',partial:'部分保存',failed:'未完成',unchanged:'值已相同',not_required:'无需重启',not_started:'未重启',not_confirmed:'未确认重启',succeeded:'已完成',different:'与本次设置不同'};
async function load(){
  const generation=++request,project=props.projectId;loading.value=true;error.value="";
  try{const value=await listBusinessOperationHistory(project,"smart_screen",{page:page.value,pageSize:20});if(generation===request&&props.projectId===project)data.value=value;}
  catch(cause){if(generation===request)error.value=commandErrorText(cause,"读取项目操作记录失败");}
  finally{if(generation===request)loading.value=false;}
}
async function open(id:string){
  const generation=++detailRequest,project=props.projectId;error.value="";
  try{const value=await getBusinessOperationHistoryDetail(project,"smart_screen",id);if(generation===detailRequest&&props.projectId===project)detail.value=value;}
  catch(cause){if(generation===detailRequest)error.value=commandErrorText(cause,"读取记录详情失败");}
}
watch(()=>props.projectId,()=>{++request;++detailRequest;page.value=1;detail.value=undefined;releasePreview.value=undefined;notice.value="";data.value={items:[],total:0,page:1,pageSize:20};void load();},{immediate:true});
onBeforeUnmount(()=>{++request;++detailRequest;});
</script>
<template>
  <section class="screen-shared-history">
    <div class="screen-inline-actions"><span class="screen-muted">本业务项目的共享结果 · 完整日志和诊断文件保存在来源电脑</span><span class="screen-spacer"></span><n-button size="small" :loading="loading" @click="load">刷新记录</n-button></div>
    <n-alert v-if="error" type="warning">{{error}}</n-alert>
    <n-alert v-if="notice" type="info">{{notice}}</n-alert>
    <div class="screen-history-layout">
      <aside class="screen-task-list inx-scroll-area">
        <button v-for="item in data.items" :key="item.id" type="button" :class="{active:detail?.operation.id===item.id}" @click="open(item.id)"><b>{{item.operationName}}</b><span>{{item.targetCount}} 台 · {{labels[item.state]??item.state}}</span><small>{{formatDisplayDateTime(item.startedAt)}} · {{item.operatorName}}</small></button>
        <p v-if="!loading&&!data.items.length" class="screen-muted">暂无已保存的项目操作记录</p>
      </aside>
      <div class="screen-history-detail screen-shared-detail">
        <template v-if="detail">
          <div class="shared-detail-heading"><h3>{{detail.operation.operationName}}</h3><n-button size="small" :loading="releaseBusy" @click="prepareRelease">查看并释放操作占用</n-button></div>
          <p v-if="detail.targets[0]?.details?.retryOfOperationId" class="screen-muted shared-long-text" :title="detail.targets[0].details.retryOfOperationId">重试来源：{{detail.targets[0].details.retryOfOperationId}}</p>
          <p v-if="detail.targets[0]?.details?.package?.name" class="screen-muted shared-long-text" :title="`SHA-256：${detail.targets[0].details.package.sha256||'未记录'}`">安装包：{{detail.targets[0].details.package.name}} · {{detail.targets[0].details.package.version}}-{{detail.targets[0].details.package.versionCode}}</p>
          <p class="screen-muted shared-detail-meta"><span>操作人：{{detail.operation.operatorName}}</span><span :title="detail.operation.instanceId">来源电脑：{{detail.operation.instanceId}}</span><span>开始时间：{{formatDisplayDateTime(detail.operation.startedAt)}}</span></p>
          <compact-operation-table :columns="targetColumns" label="项目共享操作结果" :reset-key="`${detail.operation.id}:${targetPage}:${targetPageSize}`">
            <tr v-for="target in visibleTargets" :key="target.resourceKey">
              <td :title="target.details?.targetName||target.resourceKey">{{target.details?.targetName||target.resourceKey}}</td>
              <td :title="target.details?.targetIp||target.details?.observedIp">{{target.details?.targetIp||target.details?.observedIp||'—'}}</td>
              <td><n-tag size="small" :bordered="false">{{labels[target.state]??target.state}}</n-tag></td>
              <td v-if="detail.operation.operationType==='app_config'" :title="`${target.details?.configuration?.fields?.map(appConfigFieldLabel).join('、')||'未记录'}\n${changeSummary(target)}`">{{target.details?.configuration?.fields?.map(appConfigFieldLabel).join('、')||'未记录'}}<small>{{changeSummary(target)}}</small></td>
              <td v-else-if="detail.operation.operationType==='ntp'" :title="`${ntpAddressSummary(target)}\n${ntpSummary(target)}`">{{ntpAddressSummary(target)}}<small>{{target.details?.ntp?.clockOffsetSeconds == null ? '未取得时间偏差' : `时间偏差 ${target.details.ntp.clockOffsetSeconds} 秒`}}</small></td>
              <td v-else :title="`${target.beforeVersion||'未记录'} → ${target.afterVersion||'未更新'}`">{{target.beforeVersion||'未记录'}} → {{target.afterVersion||'未更新'}}</td>
              <td :title="targetSummary(target)">{{target.resultSummary||target.errorSummary||'—'}}<small v-if="target.details?.ntp">{{ntpSummary(target)}}</small><small v-else-if="target.details?.configuration">{{configSummary(target)}}</small><small v-else-if="target.details">设备：{{labels[target.details.device??'not_required']}} · 平台数据：{{labels[target.details.business??'not_required']}}</small></td>
            </tr>
            <tr v-if="!detail.targets.length"><td :colspan="targetColumns.length" class="screen-muted">暂无逐台结果</td></tr>
          </compact-operation-table>
          <div class="shared-target-pages"><span>共 {{detail.targets.length}} 台</span><n-pagination v-model:page="targetPage" v-model:page-size="targetPageSize" :page-sizes="[20,50,100]" :item-count="detail.targets.length" :page-slot="5" show-size-picker size="small" /></div>
        </template>
        <div v-else class="screen-empty">选择一条记录查看结果。共享记录不会直接重做设备操作。</div>
      </div>
    </div>
    <div class="shared-record-pages"><span class="screen-muted">共 {{data.total}} 条操作记录</span><n-pagination v-model:page="page" :page-size="20" :item-count="data.total" size="small" @update:page="load" /></div>
    <n-modal :show="!!releasePreview" preset="card" title="确认强制释放占用" style="width:min(620px,94vw)" :mask-closable="false" @close="releasePreview=undefined">
      <p>请确认现在可以解除操作锁。解除后，对方本次操作不能继续提交更改；已经发给设备的命令可能仍在执行，释放不会撤销这些命令。</p>
      <compact-operation-table v-if="releasePreview" class="shared-release-table" :columns="lockColumns" label="待释放占用" :style="{height:Math.min(280,32+releasePreview.locks.length*36)+'px'}"><tr v-for="lock in releasePreview.locks" :key="lock.resourceKey"><td :title="lock.resourceKey">{{lock.resourceType==='smart_screen_registry'?'平台登记范围':lock.resourceKey}}</td><td :title="lock.ownerUser">{{lock.ownerUser}}</td><td :title="lock.ownerInstanceId">电脑 {{lock.ownerInstanceId}}</td></tr></compact-operation-table>
      <template #footer><div class="screen-inline-actions"><n-button :disabled="releaseBusy" @click="releasePreview=undefined">取消</n-button><n-button type="warning" :loading="releaseBusy" @click="confirmRelease">已确认，强制释放</n-button></div></template>
    </n-modal>
  </section>
</template>
<style scoped>
.screen-shared-history{display:flex;flex:1;flex-direction:column;gap:8px;min-height:0;overflow:hidden}
.screen-shared-history>.screen-inline-actions,.screen-shared-history>.n-alert{flex:none}
.screen-shared-history .screen-history-layout{flex:1;min-height:0;grid-template-columns:200px minmax(0,1fr)}
.screen-shared-detail{display:flex;flex-direction:column;gap:8px;min-height:0;overflow:hidden;padding:10px}
.shared-detail-heading{display:flex;flex:none;justify-content:space-between;gap:8px;align-items:center}.shared-detail-heading h3{margin:0;font-size:14px}
.shared-detail-meta{display:flex;flex-wrap:wrap;gap:3px 12px}.shared-detail-meta span{max-width:100%;overflow:hidden;text-overflow:ellipsis;white-space:nowrap}
.screen-shared-detail>p{flex:none;margin:0;font-size:11px;line-height:18px}.shared-long-text{overflow:hidden;text-overflow:ellipsis;white-space:nowrap}
.shared-target-pages,.shared-record-pages{display:flex;flex:none;align-items:center;justify-content:space-between;gap:8px;font-size:11px;min-height:28px}
.screen-shared-history .screen-task-list{padding:8px}.screen-shared-history .screen-task-list button{padding:8px;gap:3px;margin-bottom:4px}
.shared-release-table{flex:none;max-height:40vh}
</style>
