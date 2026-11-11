// Stage only reviewed public artifacts; never copy a profile or signing key.
import fs from 'node:fs';
import path from 'node:path';
import crypto from 'node:crypto';
import {execFileSync} from 'node:child_process';
import {fileURLToPath} from 'node:url';
const project=path.resolve(path.dirname(fileURLToPath(import.meta.url)),'..');
const args=process.argv.slice(2),i=args.indexOf('--output'),output=i<0?null:path.resolve(args[i+1]||'');
if(!output||fs.existsSync(output))throw Error('Specify a fresh --output directory');
const read=name=>JSON.parse(fs.readFileSync(path.join(project,name),'utf8').replace(/^\uFEFF/,''));
const sha=file=>crypto.createHash('sha256').update(fs.readFileSync(file)).digest('hex').toUpperCase();
const build=read('RELEASE_BUILD.json'),quality=read('QUALITY_GATE_RESULTS.json'),audit=read('PACKAGE_AUDIT_RESULTS.json'),summary=read('STORAGE_RELEASE_TEST_RESULTS.json');
const version=build.version;
if(!/^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)$/.test(version||''))throw Error('Invalid release version');
if(!quality.passed||!audit.passed||!summary.passed||!build.currentReports||!Number.isInteger(build.releaseSummaryGroupsPassed)||build.releaseSummaryGroupsPassed<12||summary.releaseSha256!==build.sha256||audit.releaseSha256!==build.sha256||quality.sourceFingerprint.sha256!==build.qualitySourceFingerprint.sha256)throw Error('Finalize all exact-candidate release checks before staging');
const actualFingerprint=JSON.parse(execFileSync('powershell.exe',['-NoProfile','-ExecutionPolicy','Bypass','-File',path.join(project,'tools/source-fingerprint.ps1'),'-SourceDirectory',project],{encoding:'utf8',windowsHide:true}).replace(/^\uFEFF/,''));
if(actualFingerprint.sha256!==quality.sourceFingerprint.sha256||sha(path.join(project,'Cargo.lock'))!==build.lockSha256)throw Error('Source changed after verification');
for(const [name,digest] of Object.entries(build.currentReports)){
 if(!/^(?:installer\/)?[A-Z0-9_]+\.json$/.test(name)||sha(path.join(project,name))!==digest)throw Error('A current release report changed after finalization');
}
const installerName=`Yas-${version}-Setup-x64.exe`,installer=path.resolve(project,'..',installerName);
if(sha(installer)!==audit.artifacts?.[installerName]?.sha256)throw Error('The installer differs from the audited release');
const sanitizedBuild={version,sha256:build.sha256,unitTestsPassed:build.unitTestsPassed,qualitySourceFingerprint:build.qualitySourceFingerprint,releaseSummaryGroupsPassed:build.releaseSummaryGroupsPassed};
const attestation={schema:1,version,releaseSha256:build.sha256,installerSha256:sha(installer),sourceFingerprint:build.qualitySourceFingerprint,lockSha256:build.lockSha256,publicKeySha256:sha(path.join(project,'release.pub')),unitTestsPassed:build.unitTestsPassed,releaseGroupsPassed:build.releaseSummaryGroupsPassed,build:sanitizedBuild,packageAudit:{version,passed:true,releaseSha256:build.sha256,artifacts:{[installerName]:audit.artifacts[installerName]}},scope:'Exact locally tested full installer; source and binary identity are rechecked by the release job. No personal profile or private preservation proof is included.'};
fs.mkdirSync(output,{recursive:true});fs.copyFileSync(installer,path.join(output,installerName));fs.writeFileSync(path.join(output,'prevalidated.json'),JSON.stringify(attestation,null,2)+'\n');
console.log(JSON.stringify({prepared:true,published:false,version,output,files:[installerName,'prevalidated.json'],installerSha256:attestation.installerSha256,privateDataIncluded:false}));
