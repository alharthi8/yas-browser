"""Fetch the reviewed CPU runtime; verify the archive and every manifest entry."""
import hashlib, json, os, pathlib, tempfile, urllib.request, zipfile

root = pathlib.Path(__file__).resolve().parent.parent / 'ai-runtime'
manifest_bytes = (root / 'AI_RUNTIME_MANIFEST.json').read_bytes()
assert hashlib.sha256(manifest_bytes).hexdigest().upper() == 'FE26D105E6D8A9B82DA1AACC652F3EC9509591560FF5EC3807150DD8375FDDC7'
manifest = json.loads(manifest_bytes)
def valid(file, entry):
    return file.is_file() and file.stat().st_size == entry['bytes'] and hashlib.file_digest(file.open('rb'), 'sha256').hexdigest().upper() == entry['sha256']
missing = [entry for entry in manifest['files'] if not valid(root / entry['path'], entry)]
if missing:
    source = manifest['sourceArchive']
    assert source['url'] == 'https://github.com/ollama/ollama/releases/download/v0.40.16/ollama-windows-amd64.zip'
    archive = pathlib.Path(tempfile.gettempdir()) / 'yas-ollama-0.40.16-reviewed.zip'
    if not archive.is_file() or archive.stat().st_size != source['bytes'] or hashlib.file_digest(archive.open('rb'), 'sha256').hexdigest().upper() != source['sha256']:
        print('Downloading pinned Ollama distribution; GPU payloads and models will not be packaged.', flush=True)
        request = urllib.request.Request(source['url'], headers={'User-Agent':'Yas-Browser-release'})
        with urllib.request.urlopen(request, timeout=120) as response, archive.open('wb') as output:
            while chunk := response.read(1024 * 1024):
                output.write(chunk)
    assert archive.stat().st_size == source['bytes']
    assert hashlib.file_digest(archive.open('rb'), 'sha256').hexdigest().upper() == source['sha256'], 'Vendor archive digest mismatch'
    with zipfile.ZipFile(archive) as bundle:
        for entry in missing:
            relative = pathlib.PurePosixPath(entry['path'])
            assert not relative.is_absolute() and '..' not in relative.parts
            assert relative.suffix.lower() in ('.exe', '.dll'), 'License files must be supplied and verified in source'
            info = bundle.getinfo(entry['path'])
            assert info.file_size == entry['bytes'] and not (info.external_attr >> 16 & 0o170000) == 0o120000
            data = bundle.read(info)
            assert hashlib.sha256(data).hexdigest().upper() == entry['sha256']
            target = root.joinpath(*relative.parts)
            target.parent.mkdir(parents=True, exist_ok=True)
            target.write_bytes(data)
for entry in manifest['files']:
    assert valid(root / entry['path'], entry), 'Runtime payload validation failed'
assert {p.relative_to(root).as_posix() for p in root.rglob('*') if p.is_file()} == {e['path'] for e in manifest['files']} | {'AI_RUNTIME_MANIFEST.json'}
print('Verified exact 33-file CPU runtime inventory; no model or profile data included.')
