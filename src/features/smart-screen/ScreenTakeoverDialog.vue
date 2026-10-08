<script setup lang="ts">
import { computed, onBeforeUnmount, ref, shallowRef, watch } from 'vue';
import { NButton, NModal } from 'naive-ui';
import { registerScreenTakeoverHandler, type ScreenTakeoverConfirmation, type ScreenTakeoverRequest } from '@/shared/api/screenTakeover';

const props=defineProps<{projectId:string;contextKey:string}>();
const pending=shallowRef<ScreenTakeoverRequest>();
const busy=ref(false);
let generation=0;
let accept:((value:ScreenTakeoverConfirmation)=>void)|undefined;
let reject:((error:Error)=>void)|undefined;
const deviceAction=computed(()=>pending.value?.conflicts.some(item=>['install','time','adb','restart','reboot'].includes(item.operationType)));
function computer(value:string){return value.replace(/-(?:[A-Fa-f0-9]{12}|未知MAC)-(?=[\da-fA-F:.]+$|未联网$)/,' · ');}
function cancel(){++generation;reject?.(new Error('已取消接手，本次操作未继续'));accept=undefined;reject=undefined;pending.value=undefined;busy.value=false;}
const unregister=registerScreenTakeoverHandler(request=>new Promise((resolve,rejectRequest)=>{
  if(request.projectId!==props.projectId||pending.value){rejectRequest(new Error('当前项目或操作已变化，请重新操作'));return;}
  pending.value=request;accept=resolve;reject=rejectRequest;
}));
function confirm(){
  if(busy.value||!pending.value||!accept)return;
  busy.value=true;
  const expected=generation,project=pending.value.projectId;
  const resolve=accept;accept=undefined;reject=undefined;
  resolve({
    assertCurrent(){if(expected!==generation||props.projectId!==project)throw new Error('页面已切换，本次操作未继续');},
    finish(){if(expected===generation){pending.value=undefined;busy.value=false;}}
  });
}
watch(()=>[props.projectId,props.contextKey],cancel);
onBeforeUnmount(()=>{cancel();unregister();});
</script>

<template>
  <n-modal :show="!!pending" preset="card" title="该智能屏正在由其他电脑操作" style="width:min(620px,94vw)" :mask-closable="false" :closable="!busy" :close-on-esc="!busy" @close="cancel" @update:show="!$event&&!busy&&cancel()">
    <div class="screen-takeover-targets inx-scroll-area">
      <div v-for="item in pending?.conflicts" :key="item.operationId" class="screen-takeover-item">
        <p><strong>{{item.targets.join('、')}}</strong></p>
        <p>正在由 <strong>{{item.ownerUser}}</strong> 在 <strong>{{computer(item.ownerInstanceId)}}</strong> 上进行“{{item.operationName}}”。</p>
        <p v-if="(item.targetCount??0)>1" class="screen-muted">该操作共 {{item.targetCount}} 台屏，接手将停止该操作后续的提交。</p>
      </div>
    </div>
    <p>接手后，对方本次操作将不能继续提交更改。请确认现在可以由你继续操作。</p>
    <p v-if="deviceAction" class="screen-warning">已经发送到屏上的操作可能仍在执行。</p>
    <p v-if="busy" class="screen-muted" role="status">正在接手并重新检查最新数据…</p>
    <template #footer><div class="screen-dialog-footer"><n-button :disabled="busy" @click="cancel">取消</n-button><n-button type="warning" :loading="busy" @click="confirm">接手并继续</n-button></div></template>
  </n-modal>
</template>
<style scoped>
.screen-takeover-targets{max-height:260px;overflow:auto}
.screen-takeover-item+.screen-takeover-item{border-top:1px solid var(--inx-color-border);padding-top:8px}
.screen-takeover-item p{overflow-wrap:anywhere}
</style>
