# Windows 1.0.0 release hold — 11 November 2026

Publication is blocked. Do not tag or publish this candidate as a downloadable release until the detection is resolved and the final candidate passes the remaining checks.

Microsoft Defender blocked the locally built executable as **Trojan:Win32/Bearfoos.B!ml**, threat ID 2147731849, severity 5. The first recorded detections occurred at 12:16:55 and 12:17:07 Asia/Dubai during packaging and synthetic QA. Security intelligence version: 1.463.236.0; engine: 1.1.26100.4. Real-time protection remained enabled. The detection's cause is unresolved; no false-positive conclusion has been established.

## Completed before the block

- Versioned Windows source snapshot from development 0.4.9, with 1.0.0 resource/installer metadata and the existing signed GitHub updater integrated.
- Compiled filter-cache metadata regenerated for the changed Cargo/source inputs.
- 135 Rust unit tests passed; formatting and strict Clippy checks passed with no source or lock changes during the gate.
- Optimized Windows build completed. Its isolated native smoke check passed WebView2 launch, loopback navigation, DOM read and button interaction, then closed the owned automation endpoint.
- Exact reviewed 33-file CPU runtime inventory and hashes verified. No model or user profile was included.

## Incomplete

The broader synthetic regression stopped at an absent extension test fixture. After preparing the missing fixture for a fresh run, Defender blocked the executable before the rerun could start. Portable packaging was also blocked when copying the executable. Installer validation, complete regression, and final signed update verification are incomplete. No 1.0.0 release tag or release assets have been published. The website's Coming soon state remains live.

The 1.0 website launch script is prepared but has not been executed or deployed. It requires a published, non-draft/non-prerelease v1.0.0 release and a reachable installer with GitHub SHA-256 metadata. Other products remain Coming soon.

## Next action

Microsoft provides a software-developer sample submission route for files suspected of being incorrectly detected: https://www.microsoft.com/en-us/wdsi/filesubmission/ . Guidance: https://learn.microsoft.com/defender-xdr/submission-guide . A submission has not been sent in this session. Protection was not disabled, exclusions were not added, and blocked files were not restored or repackaged to evade detection.

Retain the detection details, source fingerprint in RELEASE_SOURCE.json, Cargo.lock, final candidate hash when available through an approved analysis process, and any Microsoft case/reference number. A clean unit-test result or dependency advisory report does not resolve a malware verdict.
