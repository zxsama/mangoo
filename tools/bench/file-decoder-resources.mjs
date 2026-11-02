// Windows-only isolated Host benchmark. Keeps source pixels constant while
// varying legal PPM trailing whitespace; records OS peak working set plus
// sampled working/private bytes, then verifies disable joins the child.
import assert from 'node:assert/strict';
import { spawn, execFile } from 'node:child_process';
import { promisify } from 'node:util';
import { mkdir, mkdtemp, open, readFile, writeFile, stat } from 'node:fs/promises';
import { createHash, randomUUID } from 'node:crypto';
import net from 'node:net';
import path from 'node:path';
import { setTimeout as delay } from 'node:timers/promises';
import sharp from 'sharp';
import { createAndActivateManagedLibrary, addExistingMediaRoot, managedLibraryHostEnv, resolveCoreBinary } from '../dev/managed-library-harness.mjs';
const exec = promisify(execFile);
assert.equal(process.platform, 'win32', 'This resource monitor uses Windows process counters');
const root=process.cwd();
const executable=resolveCoreBinary({ root });
await mkdir('.tmp/all-file-library-validation',{recursive:true});
const dir=await mkdtemp(path.join(root,'.tmp/all-file-library-validation/decoder-resources-'));
const report={dir,passed:false,platform:process.platform,coreSha256:createHash('sha256').update(await readFile(executable)).digest('hex'),runs:[]};
const wait=async(fn,label)=>{const end=Date.now()+30000;let last;while(Date.now()<end){try{const r=await fn();if(r)return r;}catch(e){last=e;}await delay(40);}throw Error(`${label}: ${last?.message}`);};
async function run(sizeMiB,index){
 const fixture=path.join(dir,String(index));await mkdir(fixture);
 const media=path.join(fixture,'source');await mkdir(media);
 const file=await open(path.join(media,'content.ppm'),'wx');
 const prefix=Buffer.from('P6\n1 1\n255\n\x11\x53\xd3','latin1');await file.write(prefix);
 const padding=Buffer.alloc(256*1024,32);let left=sizeMiB*1024*1024-prefix.length;
 while(left){const length=Math.min(left,padding.length);await file.write(padding,0,length);left-=length;}await file.close();
 const port=await new Promise(resolve=>{const s=net.createServer();s.listen(0,'127.0.0.1',()=>{const p=s.address().port;s.close(()=>resolve(p));});});
 const token=randomUUID(),base=`http://127.0.0.1:${port}/api`;let revision;
 const headers=()=>({'x-mangoo-session':token,'content-type':'application/json',...(revision?{'x-mangoo-context-revision':String(revision)}:{})});
 const api=async(route,body,method=body===undefined?'GET':'POST')=>{const r=await fetch(base+route,{method,headers:headers(),body:body===undefined?undefined:JSON.stringify(body)});const text=await r.text();assert.ok(r.ok,`${r.status} ${route}: ${text}`);return text?JSON.parse(text):null;};
 const child=spawn(executable,[],{cwd:root,windowsHide:true,env:{...process.env,...managedLibraryHostEnv(fixture),MANGOO_CORE_ADDR:`127.0.0.1:${port}`,MANGOO_SESSION_TOKEN:token,MANGOO_SERVE_WEB:'0',MANGOO_PLUGINS_DIR:process.env.MANGOO_PLUGINS_DIR ?? path.join(path.dirname(resolveCoreBinary({ root })), 'bundled-plugins'),MANGOO_PLUGIN_STATE_DIR:path.join(fixture,'plugin-state'),MANGOO_PARENT_PID:String(process.pid)},stdio:['ignore','pipe','pipe']});
 let log='',monitor;
 child.stdout.on('data',b=>log+=b);child.stderr.on('data',b=>log+=b);
 const stop=path.join(fixture,'monitor.stop');
 try{
  await wait(()=>api('/health'),'Core');
  const library=await createAndActivateManagedLibrary({baseUrl:base,token,name:'Resource fixture',databasePath:path.join(fixture,'Library','Database'),contentRoots:[{path:path.join(fixture,'Library','Files'),createIfMissing:true}]});revision=library.contextRevision;
  await addExistingMediaRoot({baseUrl:base,token,contextRevision:revision,mediaRoot:media});
  const item=await wait(async()=>{const p=await api('/media?limit=10');return p.items.length===1?p.items[0]:null;},'file');
  assert.equal((await api(`/media/${item.id}/thumbnail`)).state,'unavailable');
  const manifest=JSON.parse(await readFile(path.join(process.env.MANGOO_PLUGINS_DIR ?? path.join(path.dirname(resolveCoreBinary({ root })), 'bundled-plugins'), 'com.mangoo.ppm-decoder', 'plugin.json')));
  await api('/plugins/com.mangoo.ppm-decoder/grants',{permissions:manifest.requestedPermissions},'PUT');await api('/plugins/com.mangoo.ppm-decoder/enable',{});
  await wait(async()=>(await api('/plugins/com.mangoo.ppm-decoder')).runtimeState==='ready','decoder');
  const processResult=await exec('powershell.exe',['-NoProfile','-Command',`Get-CimInstance Win32_Process -Filter 'ParentProcessId = ${child.pid}' | Where-Object Name -eq 'mangoo-ppm-decoder.exe' | Select-Object -ExpandProperty ProcessId`],{windowsHide:true});
  const decoderId=Number(processResult.stdout.trim());assert.ok(Number.isInteger(decoderId)&&decoderId>0);
  const ready=path.join(fixture,'monitor.ready'),out=path.join(fixture,'memory.json');
  monitor=spawn('powershell.exe',['-NoProfile','-File',path.join(root,'tools/bench/file-decoder-resource-monitor.ps1'),'-CoreId',String(child.pid),'-DecoderId',String(decoderId),'-Ready',ready,'-Stop',stop,'-Output',out],{windowsHide:true,stdio:['ignore','pipe','pipe']});
  let monitorLog='';monitor.stderr.on('data',b=>monitorLog+=b);
  await wait(()=>stat(ready),'memory monitor');
  const started=performance.now();
  await wait(async()=>{const t=await api(`/media/${item.id}/thumbnail?priority=selected`);if(t.state==='failed')throw Error(t.error);return t.state==='ready';},'thumbnail');
  const elapsedMs=performance.now()-started;
  const response=await fetch(base+`/media/${item.id}/thumbnail/blob`,{headers:headers()});assert.equal(response.status,200);
  const {info}=await sharp(Buffer.from(await response.arrayBuffer())).raw().toBuffer({resolveWithObject:true});assert.equal(info.width,1);assert.equal(info.height,1);
  await delay(100);await writeFile(stop,'stop');
  await new Promise((resolve,reject)=>monitor.once('exit',code=>code===0?resolve():reject(Error(monitorLog))));
  const memory=JSON.parse((await readFile(out,'utf8')).replace(/^\uFEFF/,''));
  const disabled=await api('/plugins/com.mangoo.ppm-decoder/disable',{});assert.equal(disabled.runtimeState,'disabled');
  await exec('powershell.exe',['-NoProfile','-Command',`if(Get-Process -Id ${decoderId} -ErrorAction SilentlyContinue){exit 3}`],{windowsHide:true});
  return {sizeMiB,elapsedMs,...memory,disableJoined:true};
 }finally{
  await writeFile(stop,'stop').catch(()=>{});
  if(monitor?.exitCode===null)monitor.kill();
  if(child.exitCode===null){child.kill();await Promise.race([new Promise(r=>child.once('exit',r)),delay(5000)]);}
  await writeFile(path.join(fixture,'core.log'),log.replaceAll(token,'[session]'));
 }
}
try{
 for(const [index,size]of [2,48,2,48].entries()){const result=await run(size,index);report.runs.push(result);console.log(JSON.stringify(result));}
 const small=report.runs.filter(r=>r.sizeMiB===2),large=report.runs.filter(r=>r.sizeMiB===48);
 const maximum=(runs,key,field)=>Math.max(...runs.map(r=>r[key][field]));
 report.comparison={inputGrowthBytes:46*1024*1024,corePeakGrowthBytes:maximum(large,'core','peakWorkingSet')-maximum(small,'core','peakWorkingSet'),decoderPeakGrowthBytes:maximum(large,'decoder','peakWorkingSet')-maximum(small,'decoder','peakWorkingSet')};
 assert.ok(report.comparison.corePeakGrowthBytes<16*1024*1024,'Host memory tracks full input size');
 assert.ok(report.comparison.decoderPeakGrowthBytes<16*1024*1024,'decoder memory tracks full input size');
 report.passed=true;
}catch(error){report.error=error.stack;process.exitCode=1;console.error(error);}finally{await writeFile(path.join(dir,'result.json'),JSON.stringify(report,null,2));console.log(`Evidence: ${dir}`);}
