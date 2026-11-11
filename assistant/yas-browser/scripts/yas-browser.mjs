// Public opt-in desktop control. Requires Node.js22+; no third-party modules.
import fs from 'node:fs';
import path from 'node:path';
import os from 'node:os';
import crypto from 'node:crypto';
import net from 'node:net';
import {spawn,execFileSync} from 'node:child_process';
const [command,...args]=process.argv.slice(2);
const option=(name)=>{const i=args.indexOf('--'+name);return i<0?undefined:args[i+1]};
const print=x=>console.log(JSON.stringify(x));
const pause=ms=>new Promise(r=>setTimeout(r,ms));
const identity=pid=>{
 const code='$p=Get-Process -Id ([int]$env:YAS_CONTROL_PID) -ErrorAction Stop; [ordered]@{pid=$p.Id;created=$p.StartTime.ToUniversalTime().Ticks.ToString();path=$p.Path}|ConvertTo-Json -Compress';
 return JSON.parse(execFileSync('powershell',['-NoProfile','-NonInteractive','-Command',code],{encoding:'utf8',windowsHide:true,env:{...process.env,YAS_CONTROL_PID:String(pid)}}));
};
function web(value){const u=new URL(value);if(!['http:','https:'].includes(u.protocol)||u.username||u.password)throw Error('An HTTP(S) URL without embedded credentials is required');return u.href}
async function cdp(target,method,params={}){
 const ws=new WebSocket(target.webSocketDebuggerUrl);
 await new Promise((r,j)=>{ws.onopen=r;ws.onerror=j});
 try{return await new Promise((resolve,reject)=>{
  const timer=setTimeout(()=>reject(Error('Browser command timed out; inspect before retrying')),12000);
  ws.onmessage=e=>{const v=JSON.parse(e.data);if(v.id===1){clearTimeout(timer);if(v.error||v.result?.exceptionDetails)reject(Error('Browser operation failed'));else resolve(v.result)}};
  ws.send(JSON.stringify({id:1,method,params}));
 })}finally{ws.close()}
}
const evaluate=async(t,expression)=>(await cdp(t,'Runtime.evaluate',{expression,returnByValue:true,userGesture:true})).result?.value;
if(command==='help'||!command){print({commands:['launch --executable PATH','tabs --session FILE','open --session FILE --url HTTPS_URL','read --session FILE --tab ID','click --session FILE --tab ID --selector CSS','fill --session FILE --tab ID --selector CSS --text VALUE','stop --session FILE'],scope:'A separate opt-in automation profile. No access to saved passwords/cookies or arbitrary JavaScript command.'});process.exit(0)}
if(command==='launch'){
 const exe=fs.realpathSync(option('executable')||path.join(process.env.LOCALAPPDATA||'', 'Programs','Yas','Yas.exe'));
 if(!fs.statSync(exe).isFile())throw Error('Yas Browser executable required');
 const directory=path.join(process.env.LOCALAPPDATA||os.tmpdir(),'Yas','Automation',crypto.randomUUID());
 fs.mkdirSync(directory,{recursive:true});const profile=path.join(directory,'profile');fs.mkdirSync(profile);
 const probe=net.createServer();await new Promise((r,j)=>{probe.once('error',j);probe.listen(0,'127.0.0.1',r)});const port=probe.address().port;await new Promise(r=>probe.close(r));
 const child=spawn(exe,['--user-data-dir',profile],{detached:true,windowsHide:false,stdio:'ignore',cwd:path.dirname(exe),env:{...process.env,WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS:`--remote-debugging-address=127.0.0.1 --remote-debugging-port=${port}`}});child.unref();
 const record={version:1,exe,profile,port,identity:identity(child.pid),createdTabs:[]};
 const session=path.join(directory,'session.json');fs.writeFileSync(session,JSON.stringify(record));
 let ready=false;for(let i=0;i<100;i++){try{const rows=await fetch(`http://127.0.0.1:${port}/json/list`,{signal:AbortSignal.timeout(1000)}).then(r=>r.json());if(rows.some(t=>t.url==='http://yas-ui.localhost/')){ready=true;break}}catch{}await pause(100)}
 if(!ready)throw Error(`Yas Browser did not expose its local control interface; session retained at ${session}`);
 print({session,profile,pid:child.pid,ready:true});process.exit(0)
}
const file=option('session');if(!file)throw Error('--session FILE is required');
const record=JSON.parse(fs.readFileSync(file,'utf8'));
if(record.version!==1||!Number.isInteger(record.port)||record.port<1||record.port>65535)throw Error('Invalid control session');
const actual=identity(record.identity.pid);
if(actual.created!==record.identity.created||actual.path.toLowerCase()!==record.exe.toLowerCase())throw Error('The original Yas Browser process has exited; start a new session');
const rows=await fetch(`http://127.0.0.1:${record.port}/json/list`,{signal:AbortSignal.timeout(2000)}).then(r=>r.json());
const shell=rows.find(t=>t.url==='http://yas-ui.localhost/');if(!shell)throw Error('Yas Browser control shell unavailable');
const content=rows.filter(t=>/^https?:/.test(t.url)&&new URL(t.url).hostname!=='yas-ui.localhost');
if(command==='tabs'){print(content.map(t=>({id:t.id,title:/accounts\.|login\./.test(new URL(t.url).hostname)?'Account sign-in':t.title,url:new URL(t.url).origin+new URL(t.url).pathname})));process.exit(0)}
if(command==='open'){
 const url=web(option('url'));await evaluate(shell,`hidePanel();send('new');send('navigate',{url:${JSON.stringify(url)}})`);
 print({requested:true,url});process.exit(0)
}
if(command==='stop'){await evaluate(shell,"send('quit')");print({exitRequested:true});process.exit(0)}
const target=content.find(t=>t.id===option('tab'));if(!target)throw Error('Choose a current tab ID from tabs');
const u=new URL(target.url);
if(/(^|\.)(accounts\.google\.com|login\.microsoftonline\.com|login\.live\.com)$/.test(u.hostname))throw Error('Authentication page: hand control to the user');
if(command==='read'){
 print(await evaluate(target,`(()=>{const text=(document.body?.innerText||'');return {title:document.title,url:location.origin+location.pathname,text:text.slice(0,16000),truncated:text.length>16000,controls:[...document.querySelectorAll('button,a,input,select,textarea')].slice(0,80).map(e=>({tag:e.tagName,type:e.type||'',name:e.getAttribute('aria-label')||e.innerText?.slice(0,120)||e.getAttribute('placeholder')||'',id:e.id}))}})()`));process.exit(0)
}
if(!['click','fill'].includes(command))throw Error('Unsupported control command');
const selector=option('selector');if(!selector||selector.length>1000)throw Error('A bounded CSS selector from a fresh observation is required');
const value=option('text');if(command==='fill'&&(typeof value!=='string'||value.length>16000))throw Error('A bounded --text value is required');
const result=await evaluate(target,`(()=>{const nodes=document.querySelectorAll(${JSON.stringify(selector)});if(nodes.length!==1)throw Error('Selector must match exactly one element');const e=nodes[0];if(!e.getClientRects().length||e.disabled)throw Error('Control unavailable');if(e.type==='password'||/password|one-time-code|cc-number/.test(e.autocomplete||''))throw Error('Sensitive authentication/payment fields require user control');${command==='fill'?`if(!['INPUT','TEXTAREA'].includes(e.tagName)||e.readOnly)throw Error('Editable input required');e.focus();e.value=${JSON.stringify(value)};e.dispatchEvent(new Event('input',{bubbles:true}));e.dispatchEvent(new Event('change',{bubbles:true}));`:'e.click();'}return {performed:true}})()`);
print(result);
