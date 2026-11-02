// Real accountless decoder/Host integration; fresh fixture files only.
import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { mkdir, mkdtemp, writeFile, readFile, stat, utimes } from 'node:fs/promises';
import { randomUUID } from 'node:crypto';
import net from 'node:net';
import path from 'node:path';
import { setTimeout as delay } from 'node:timers/promises';
import sharp from 'sharp';
import { createAndActivateManagedLibrary, addExistingMediaRoot, managedLibraryHostEnv, resolveCoreBinary } from '../dev/managed-library-harness.mjs';
const root = process.cwd();
await mkdir('.tmp/all-file-library-validation', { recursive:true });
const dir = await mkdtemp(path.join(root,'.tmp/all-file-library-validation/decoder-api-'));
const media = path.join(dir,'media'); await mkdir(media);
const p6 = (rgb, width=2048, height=1024) => {
  const pixels = Buffer.alloc(width*height*3); for(let i=0;i<pixels.length;i+=3) pixels.set(rgb,i);
  return Buffer.concat([Buffer.from(`P6\n${width} ${height}\n255\n`),pixels]);
};
await writeFile(path.join(media,'red.ppm'), 'P3\n2 1\n255\n255 0 0 255 0 0\n');
await writeFile(path.join(media,'blue.ppm'), p6([0,0,255]));
await writeFile(path.join(media,'invalid.ppm'), 'P3\n1 1\n15\n1 2 3\n');
const port = await new Promise(resolve=>{ const s=net.createServer();s.listen(0,'127.0.0.1',()=>{const p=s.address().port;s.close(()=>resolve(p));});});
const token=randomUUID(), base=`http://127.0.0.1:${port}/api`;
let log='', revision, child;
const headers=()=>({'x-mangoo-session':token,'content-type':'application/json',...(revision?{'x-mangoo-context-revision':String(revision)}:{})});
const api=async(route, body, method=body===undefined?'GET':'POST')=>{
 const r=await fetch(base+route,{method,headers:headers(),body:body===undefined?undefined:JSON.stringify(body)});
 const text=await r.text();assert.ok(r.ok,`${method} ${route}: ${r.status} ${text}`);return text?JSON.parse(text):null;
};
const wait=async(check,label,ms=30000)=>{let last;const end=Date.now()+ms;while(Date.now()<end){try{const value=await check();if(value)return value;}catch(e){last=e;}await delay(100);}throw Error(`${label}: ${last?.message}`);};
const thumb=(id)=>api(`/media/${id}/thumbnail?priority=selected`);
const blob=async(id,preview=false)=>{
 const r=await fetch(base+`/media/${id}/${preview?'preview?viewportLongPx=2048':'thumbnail/blob'}`,{headers:headers()});
 assert.equal(r.status,200);return {bytes:Buffer.from(await r.arrayBuffer()),tier:r.headers.get('x-mangoo-preview-tier')};
};
const color=async(bytes, channel)=>{const {data,info}=await sharp(bytes).raw().toBuffer({resolveWithObject:true});const means=[0,0,0];for(let i=0;i<data.length;i+=info.channels)for(let c=0;c<3;c++)means[c]+=data[i+c];for(let c=0;c<3;c++)means[c]/=info.width*info.height;assert.ok(means[channel]>245 && means.filter((_,i)=>i!==channel).every(n=>n<12),String(means));return {width:info.width,height:info.height,means};};
const report={dir,passed:false};
try{
 child=spawn(resolveCoreBinary({ root }),[],{cwd:root,windowsHide:true,env:{...process.env,...managedLibraryHostEnv(dir),MANGOO_CORE_ADDR:`127.0.0.1:${port}`,MANGOO_SESSION_TOKEN:token,MANGOO_SERVE_WEB:'0',MANGOO_PLUGINS_DIR:process.env.MANGOO_PLUGINS_DIR ?? path.join(path.dirname(resolveCoreBinary({ root })), 'bundled-plugins'),MANGOO_PLUGIN_STATE_DIR:path.join(dir,'plugin-state'),MANGOO_PARENT_PID:String(process.pid)},stdio:['ignore','pipe','pipe']});
 child.stdout.on('data',b=>log+=b);child.stderr.on('data',b=>log+=b);
 await wait(()=>api('/health'),'Core');
 const library=await createAndActivateManagedLibrary({baseUrl:base,token,name:'Decoder fixture',databasePath:path.join(dir,'Library','Database'),contentRoots:[{path:path.join(dir,'Library','Files'),createIfMissing:true}]});revision=library.contextRevision;
 await addExistingMediaRoot({baseUrl:base,token,contextRevision:revision,mediaRoot:media});
 const files=await wait(async()=>{const p=await api('/media?limit=100');return p.items.length===3?p.items:null;},'scan');
 const red=files.find(f=>f.name==='red.ppm'), blue=files.find(f=>f.name==='blue.ppm'), invalid=files.find(f=>f.name==='invalid.ppm');
 assert.equal((await thumb(red.id)).state,'unavailable');assert.equal(red.kind,null);
 const plugin='/plugins/com.mangoo.ppm-decoder';const manifest=JSON.parse(await readFile(path.join(process.env.MANGOO_PLUGINS_DIR ?? path.join(path.dirname(resolveCoreBinary({ root })), 'bundled-plugins'), 'com.mangoo.ppm-decoder', 'plugin.json')));
 await api(plugin+'/enable',{});await api(plugin+'/grants',{permissions:manifest.requestedPermissions},'PUT');
 await wait(async()=>{const p=await api(plugin);assert.equal(p.accountUuid,null);return p.runtimeState==='ready';},'decoder ready');
 await wait(async()=>{const t=await thumb(red.id);return t.state==='ready'?t:null;},'red thumbnail');
 await wait(async()=>{const t=await thumb(blue.id);return t.state==='ready'?t:null;},'blue thumbnail');
 report.red=await color((await blob(red.id)).bytes,0);report.blue=await color((await blob(blue.id)).bytes,2);
 const preview=await blob(blue.id,true);assert.equal(preview.tier,'preview_3072');report.preview=await color(preview.bytes,2);assert.equal(report.preview.width,2048);
 const failed=await wait(async()=>{const t=await thumb(invalid.id);return t.state==='failed'?t:null;},'invalid fallback');
 for(let i=0;i<5;i++)assert.deepEqual(await thumb(invalid.id),failed);
 report.negativeCacheStable = true;
 const old=await blob(red.id); const disabled=await api(plugin+'/disable',{});assert.equal(disabled.runtimeState,'disabled');
 assert.deepEqual((await blob(red.id)).bytes,old.bytes);assert.deepEqual((await blob(blue.id,true)).bytes,preview.bytes);
 await writeFile(path.join(media,'red.ppm'),p6([0,255,0],3,2));
 await wait(async()=>{const r=await api(`/media/${red.id}`);return r.size!==red.size&&r.presentation.thumbnail==='unavailable';},'changed disabled fallback');
 assert.equal((await thumb(red.id)).state,'unavailable');
 assert.equal((await fetch(base+`/media/${red.id}/thumbnail/blob`,{headers:headers()})).status,404);
 await api(plugin+'/enable',{});await wait(async()=>(await api(plugin)).runtimeState==='ready','reenable');
 await wait(async()=>(await thumb(red.id)).state==='ready','changed new thumbnail');report.changed=await color((await blob(red.id)).bytes,1);
 const retryFailure=await wait(async()=>{const t=await thumb(invalid.id);return t.state==='failed'?t:null;},'failure retries in new lifecycle');
 assert.ok(retryFailure.updatedAt > failed.updatedAt);
 report.lifecycleRetriesNegativeCache=true;
 await api(plugin+'/disable',{});
 const sameBefore = await stat(path.join(media,'red.ppm'));
 const catalogBefore = await api(`/media/${red.id}`);
 await writeFile(path.join(media,'red.ppm'),p6([0,0,255],3,2));
 await utimes(path.join(media,'red.ppm'),sameBefore.atime,sameBefore.mtime);
 await wait(async()=>(await thumb(red.id)).state==='unavailable','same-metadata content change invalidates cache');
 const sameAfter=await api(`/media/${red.id}`);assert.equal(sameAfter.size,catalogBefore.size);assert.equal(sameAfter.mtime,catalogBefore.mtime);
 assert.equal((await fetch(base+`/media/${red.id}/thumbnail/blob`,{headers:headers()})).status,404);
 await api(plugin+'/enable',{});await wait(async()=>(await api(plugin)).runtimeState==='ready','same-metadata reenable');
 await wait(async()=>(await thumb(red.id)).state==='ready','same-metadata replacement');
 report.sameMetadataChanged=await color((await blob(red.id)).bytes,2);
 report.sourceIdentityPreserved=(await api(`/media/${red.id}`)).id===red.id;
 assert.equal(report.sourceIdentityPreserved,true);assert.equal((await api('/media?limit=100')).items.length,3);
 report.passed=true;console.log(JSON.stringify(report,null,2));
}catch(error){report.error=error.stack;console.error(error);}finally{
 if(child?.exitCode===null){child.kill();await Promise.race([new Promise(r=>child.once('exit',r)),delay(5000)]);}
 await writeFile(path.join(dir,'core.log'),log.replaceAll(token,'[session]'));await writeFile(path.join(dir,'result.json'),JSON.stringify(report,null,2));
 console.log('Evidence: '+dir);if(!report.passed)process.exitCode=1;
}
