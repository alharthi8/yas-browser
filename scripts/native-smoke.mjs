// Launches only a separate automation profile and a loopback fixture, never the user's browsing profile.
import {createServer} from 'node:http';
import {spawnSync} from 'node:child_process';
import path from 'node:path';
import fs from 'node:fs/promises';
const executable=path.resolve(process.argv[2]||'target/release/yas.exe');
const cli=path.resolve('assistant/yas-browser/scripts/yas-browser.mjs');
function run(...args){const p=spawnSync(process.execPath,[cli,...args],{encoding:'utf8',windowsHide:true,timeout:30000});if(p.status!==0)throw Error('Native automation failed: '+p.stderr.slice(0,500));return JSON.parse(p.stdout.trim());}
const server=createServer((req,res)=>{res.setHeader('Content-Type','text/html');res.end('<!doctype html><title>Yas release fixture</title><h1>Yas native launch verified</h1><button id="smoke-button" onclick="document.querySelector(\'#result\').textContent=\'Native click verified\'">Check interaction</button><p id="result">Waiting</p>');});
await new Promise(r=>server.listen(0,'127.0.0.1',r));
let session;
try{
 const launch=run('launch','--executable',executable);session=launch.session;
 if(!session)throw Error('Automation session was not returned');
 run('open','--session',session,'--url',`http://127.0.0.1:${server.address().port}/`);
 let tab,read;
 for(let i=0;i<30;i++){
  await new Promise(r=>setTimeout(r,300));
  tab=run('tabs','--session',session).find(t=>t.title==='Yas release fixture');
  if(tab){read=run('read','--session',session,'--tab',tab.id);if(read.text.includes('Yas native launch verified'))break;}
 }
 if(!tab||!read?.text.includes('Yas native launch verified'))throw Error('Native webview did not load the release fixture');
 if(!read.controls.some(c=>c.id==='smoke-button'))throw Error('Fixture control is absent');
 run('click','--session',session,'--tab',tab.id,'--selector','#smoke-button');
 const clicked=run('read','--session',session,'--tab',tab.id);
 if(!clicked.text.includes('Native click verified'))throw Error('Native interaction did not complete');
 console.log('YAS_NATIVE_SMOKE_OK: WebView2 launch, fixture navigation, DOM read and interaction');
}finally{
 if(session){run('stop','--session',session);const owned=JSON.parse(await fs.readFile(session,'utf8'));let closed=false;for(let i=0;i<40;i++){await new Promise(r=>setTimeout(r,150));try{const r=await fetch(`http://127.0.0.1:${owned.port}/json/version`,{signal:AbortSignal.timeout(500)});if(!r.ok){closed=true;break;}}catch{closed=true;break;}}if(!closed)throw Error('Owned automation endpoint did not close');}
 await new Promise(r=>server.close(r));
}
