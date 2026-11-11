import fs from 'node:fs';
import path from 'node:path';
import os from 'node:os';
import crypto from 'node:crypto';
import {spawnSync} from 'node:child_process';
import {fileURLToPath} from 'node:url';
const project=path.resolve(path.dirname(fileURLToPath(import.meta.url)),'..');
const source=path.join(project,'tools','publish-update.mjs'),fixture=fs.mkdtempSync(path.join(os.tmpdir(),'yas-publisher-test-'));
const sha=data=>crypto.createHash('sha256').update(data).digest('hex').toUpperCase();
const seed=Buffer.alloc(32,17),secret=Buffer.concat([Buffer.from('302e020100300506032b657004220420','hex'),seed]);
const key=crypto.createPrivateKey({key:secret,format:'der',type:'pkcs8'}),publicKey=crypto.createPublicKey(key);
fs.mkdirSync(path.join(fixture,'tools'));fs.copyFileSync(source,path.join(fixture,'tools','publish-update.mjs'));
fs.writeFileSync(path.join(fixture,'release.pub'),publicKey.export({format:'der',type:'spki'}).subarray(-32));
const installer=path.join(fixture,'Yas-0.4.9-Setup-x64.exe'),payload=Buffer.from('MZ synthetic publisher fixture; never execute');fs.writeFileSync(installer,payload);
const baseQuality={version:'0.4.9',passed:true,sourceUnchanged:true,lockUnchanged:true,unitTestsPassed:142,sourceFingerprint:{sha256:'A'.repeat(64)},audit:{requested:true,vulnerabilityCategoryCount:0},steps:Object.fromEntries(['format','strictClippy','lockedTests','advisoryAudit'].map(name=>[name,{passed:true}]))};
const baseBuild={version:'0.4.9',sha256:'B'.repeat(64),unitTestsPassed:142,qualitySourceFingerprint:{sha256:'A'.repeat(64)},releaseSummaryGroupsPassed:12};
const baseAudit={version:'0.4.9',passed:true,releaseSha256:'B'.repeat(64),artifacts:{[path.basename(installer)]:{sha256:sha(payload)}}};
function reports(quality=baseQuality,build=baseBuild,audit=baseAudit){for(const [name,data] of [['QUALITY_GATE_RESULTS.json',quality],['RELEASE_BUILD.json',build],['PACKAGE_AUDIT_RESULTS.json',audit]])fs.writeFileSync(path.join(fixture,name),JSON.stringify(data));}
let sequence=0;function run({version='0.4.9',signingKey=seed,output,notes}={}){
 output??=path.join(fixture,'output-'+(++sequence));
 const args=[path.join(fixture,'tools','publish-update.mjs'),'--prepare','--version',version,'--installer',installer,'--output',output];if(notes)args.push('--notes',notes);
 const result=spawnSync(process.execPath,args,{encoding:'utf8',env:{SystemRoot:process.env.SystemRoot,TEMP:os.tmpdir(),TMP:os.tmpdir(),YAS_UPDATE_SIGNING_KEY:signingKey.toString('base64')}});
 if((result.stdout+result.stderr).includes(seed.toString('base64')))throw Error('Synthetic key appeared in publisher output');
 return {...result,output};
}
const checks=[];function check(name,passed){checks.push({name,passed});if(!passed)throw Error(name)}
try{
 reports();const valid=run();check('Complete fixture evidence produces a signed release without publishing',valid.status===0&&JSON.parse(valid.stdout).published===false);
 const envelope=JSON.parse(fs.readFileSync(path.join(valid.output,'yas-windows-stable.json'))),bytes=Buffer.from(envelope.payload,'base64'),release=JSON.parse(bytes);
 check('Prepared metadata signature verifies and binds the exact installer size and hash',crypto.verify(null,bytes,publicKey,Buffer.from(envelope.signature,'base64'))&&release.bytes===payload.length&&release.sha256.toUpperCase()===sha(payload)&&release.version==='0.4.9');
 check('Only the public installer and signed feed are staged',fs.readdirSync(valid.output).sort().join('|')==='Yas-0.4.9-Setup-x64.exe|yas-windows-stable.json');
 check('A different signing identity is rejected before staging',run({signingKey:Buffer.alloc(32,18)}).status!==0);
 check('A release version differing from the tested version is rejected',run({version:'0.4.10'}).status!==0);
 reports({...baseQuality,sourceUnchanged:false});check('Changed source cannot authorize a release',run().status!==0);
 reports(baseQuality,{...baseBuild,releaseSummaryGroupsPassed:undefined});check('Missing native release validation prevents signing',run().status!==0);
 reports(baseQuality,baseBuild,{...baseAudit,artifacts:{[path.basename(installer)]:{sha256:'0'.repeat(64)}}});check('Installer digest disagreement prevents signing',run().status!==0);
 reports(baseQuality,{...baseBuild,qualitySourceFingerprint:{sha256:'C'.repeat(64)}});check('Stale source evidence prevents signing',run().status!==0);
 reports();const notes=path.join(fixture,'notes.txt');fs.writeFileSync(notes,'💙'.repeat(1025));check('Oversized Unicode release notes are rejected by UTF-8 byte length',run({notes}).status!==0);
 check('An existing prepared release is never overwritten',run({output:valid.output}).status!==0&&fs.readFileSync(path.join(valid.output,'Yas-0.4.9-Setup-x64.exe')).equals(payload));
 reports({...baseQuality,lockUnchanged:false});check('Changed dependency lockfile prevents signing',run().status!==0);
 const report={version:'0.4.9',sourceOnly:true,syntheticKeysOnly:true,appsLaunched:false,published:false,publisherSha256:sha(fs.readFileSync(source)),checks,passed:true};
 fs.writeFileSync(path.join(project,'UPDATE_PUBLISHER_TEST_RESULTS.json'),JSON.stringify(report,null,2));console.log(JSON.stringify({passed:true,checks:checks.length,published:false,syntheticKeysOnly:true}));
}finally{
 seed.fill(0);secret.fill(0);
 const resolved=fs.realpathSync(fixture);if(path.dirname(resolved)!==fs.realpathSync(os.tmpdir())||!path.basename(resolved).startsWith('yas-publisher-test-'))throw Error('Fixture cleanup scope mismatch');fs.rmSync(resolved,{recursive:true});
}
