import assert from 'node:assert/strict';
import { test } from 'node:test';
import { mkdirSync, mkdtempSync, writeFileSync, readFileSync, readdirSync, rmSync } from 'node:fs';
import { join } from 'node:path';
import { tmpdir } from 'node:os';
import { gunzipSync } from 'node:zlib';
import { Buffer } from 'node:buffer';
import process from 'node:process';
import { packageAdb } from './package-screen-tools.mjs';

function fixture(t) {
  const root=mkdtempSync(join(tmpdir(),'inx-adb-package-'));t.after(()=>rmSync(root,{recursive:true,force:true}));
  const sdk=join(root,'sdk');const source=join(sdk,'platform-tools');mkdirSync(source,{recursive:true});
  const windows=process.platform==='win32', bytes=Buffer.alloc(128);
  if(windows){bytes.write('MZ');bytes.writeUInt32LE(64,0x3c);bytes.write('PE\0\0',64,'binary');bytes.writeUInt16LE(0x8664,68);}
  else if(process.platform==='darwin'){bytes.writeUInt32LE(0xfeedfacf,0);bytes.writeUInt32LE(process.arch==='arm64'?0x0100000c:0x01000007,4);}
  else{bytes.writeUInt32BE(0x7f454c46);bytes[4]=2;bytes[5]=1;bytes.writeUInt16LE(62,18);}
  writeFileSync(join(source,windows?'adb.exe':'adb'),bytes);
  for(const name of ['NOTICE.txt','source.properties',...(windows?['AdbWinApi.dll','AdbWinUsbApi.dll']:[])])writeFileSync(join(source,name),name==='source.properties'?'Pkg.Revision=35.0.2':'fixture');
  writeFileSync(join(source,'fastboot.exe'),'unused');writeFileSync(join(source,'mke2fs.exe'),'unused');
  return {root,sdk,output:join(root,'output'),run:()=>({status:0,stdout:'ADB version'})};
}
test('工具包只保留ADB与许可证，清单校验能发现损坏',t=>{
  const f=fixture(t);const root=packageAdb(f);
  assert.deepEqual(readdirSync(root).sort(),['android','android-manifest.json']);
  assert.equal(readdirSync(join(root,'android')).some(name=>name.includes('fastboot')||name.includes('mke2fs')),false);
  const manifest=JSON.parse(readFileSync(join(root,'android-manifest.json')));assert.equal(manifest.formatVersion,2);assert.equal(manifest.versions.javaRuntime,undefined);
  packageAdb({...f,verify:true});writeFileSync(join(root,'android/NOTICE.txt'),'corrupted');assert.throws(()=>packageAdb({...f,verify:true}),/校验失败/);
});
test('Windows压缩资源包含完整的ADB和清单，可嵌入EXE',{skip:process.platform!=='win32'},t=>{
  const f=fixture(t);const archive=join(f.root,'adb.json.gz');packageAdb({...f,embeddedOutput:archive});
  const bundle=JSON.parse(gunzipSync(readFileSync(archive)));assert.equal(bundle.formatVersion,1);
  assert.equal(bundle.files.length,6);assert.ok(bundle.files.some(f=>f.path==='android/adb.exe'));assert.ok(bundle.files.some(f=>f.path==='android-manifest.json'));
});
