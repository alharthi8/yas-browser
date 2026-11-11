// Run only in the reviewed release job. The existing GitHub signing secret is
// consumed in memory; only its already-public verification key ships to users.
import fs from 'node:fs';
import path from 'node:path';
import crypto from 'node:crypto';
import {fileURLToPath} from 'node:url';
const project=path.resolve(path.dirname(fileURLToPath(import.meta.url)),'..');
const args=process.argv.slice(2),option=name=>{const i=args.indexOf(name);return i<0?undefined:args[i+1]};
const sha=data=>crypto.createHash('sha256').update(data).digest('hex');
if(!args.includes('--prepare')){
 console.log('Prepare a tested release in the protected release job: node tools/publish-update.mjs --prepare --version VERSION --installer PATH --output FRESH_DIRECTORY [--notes TEXT_FILE]. Uses the existing YAS_UPDATE_SIGNING_KEY environment secret; never pass keys in command arguments.');
 process.exit(0);
}
const installer=path.resolve(option('--installer')||''),output=path.resolve(option('--output')||''),version=option('--version');
if(!/^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)$/.test(version||'')||version.split('.').some(n=>Number(n)>65535))throw Error('A canonical three-component version is required');
if(!option('--installer')||!option('--output')||fs.existsSync(output))throw Error('Use an explicit installer and fresh output directory');
const read=name=>JSON.parse(fs.readFileSync(path.join(project,name),'utf8').replace(/^\uFEFF/,''));
const quality=read('QUALITY_GATE_RESULTS.json'),build=read('RELEASE_BUILD.json'),audit=read('PACKAGE_AUDIT_RESULTS.json');
if(quality.sourceUnchanged!==true||quality.lockUnchanged!==true||quality.audit?.requested!==true||quality.audit?.vulnerabilityCategoryCount!==0||!['format','strictClippy','lockedTests','advisoryAudit'].every(step=>quality.steps?.[step]?.passed===true)||!Number.isSafeInteger(quality.unitTestsPassed)||quality.unitTestsPassed<1||quality.unitTestsPassed!==build.unitTestsPassed||quality.sourceFingerprint?.sha256!==build.qualitySourceFingerprint?.sha256||!build.qualitySourceFingerprint?.sha256||(!Number.isSafeInteger(build.releaseSummaryGroupsPassed)||build.releaseSummaryGroupsPassed<12))throw Error('Complete, unchanged source and native release validation is required before signing');
const installerBytes=fs.readFileSync(installer),digest=sha(installerBytes);
if(installerBytes.length===0||installerBytes.length>150*1024*1024||installerBytes.subarray(0,2).toString()!=='MZ')throw Error('Invalid Windows installer');
if(!quality.passed||quality.version!==version||!audit.passed||audit.version!==version||build.version!==version||build.sha256!==audit.releaseSha256||audit.artifacts?.[path.basename(installer)]?.sha256?.toLowerCase()!==digest)throw Error('Fresh passing build, quality and package audit evidence is required');
const notes=option('--notes')?fs.readFileSync(path.resolve(option('--notes')),'utf8'):'Yas Browser '+version;
if(Buffer.byteLength(notes)>4096)throw Error('Release notes must be at most4096UTF-8 bytes');
const now=Math.floor(Date.now()/1000),payload=Buffer.from(JSON.stringify({schema:1,product:'yas-browser',platform:'windows-x86_64',version,url:`https://github.com/alharthi8/yas-browser/releases/download/v${version}/Yas-${version}-Setup-x64.exe`,sha256:digest,bytes:installerBytes.length,issued_at:now,expires_at:now+90*86400,notes}));
if(!process.env.YAS_UPDATE_SIGNING_KEY)throw Error('The protected release job must provide the existing signing secret. Do not create a replacement key.');
const raw=Buffer.from(process.env.YAS_UPDATE_SIGNING_KEY,'base64');delete process.env.YAS_UPDATE_SIGNING_KEY;
if(![32,64].includes(raw.length))throw Error('Unsupported existing signing-key format');
const secret=Buffer.concat([Buffer.from('302e020100300506032b657004220420','hex'),raw.subarray(0,32)]);
try{
 const key=crypto.createPrivateKey({key:secret,format:'der',type:'pkcs8'});
 const publicKey=crypto.createPublicKey(key).export({format:'der',type:'spki'}).subarray(-32);
 if(!crypto.timingSafeEqual(publicKey,fs.readFileSync(path.join(project,'release.pub'))))throw Error('Signing key does not match the existing browser trust key');
 const signature=crypto.sign(null,payload,key);
 fs.mkdirSync(output,{recursive:true});
 fs.writeFileSync(path.join(output,'yas-windows-stable.json'),JSON.stringify({payload:payload.toString('base64'),signature:signature.toString('base64')})+'\n',{flag:'wx'});
 fs.writeFileSync(path.join(output,`Yas-${version}-Setup-x64.exe`),installerBytes,{flag:'wx'});
}finally{secret.fill(0);raw.fill(0)}
console.log(JSON.stringify({prepared:true,version,output,installerSha256:digest,privateKeyIncluded:false,published:false}));
