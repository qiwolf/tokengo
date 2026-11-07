"""Packaging contract tests: synthetic executables are never run, no Rust build."""
from pathlib import Path
import contextlib
import hashlib
import importlib.util
import io
import json
import re
import tempfile
import unittest
from unittest import mock

HERE=Path(__file__).resolve().parent
SOURCE_ROOT=HERE.parent

def load(name,path):
    spec=importlib.util.spec_from_file_location(name,path)
    module=importlib.util.module_from_spec(spec);spec.loader.exec_module(module)
    return module

package=load('tested_package',HERE/'package.py')
manage=load('tested_package_manage',HERE/'manage.py')
COMMIT='a'*40
SOURCES=['request_paths.sql','account_overview.sql','query_performance.sql','model_invocation_totals.sql']


class PackageTests(unittest.TestCase):
    def setUp(self):
        self.temp=tempfile.TemporaryDirectory();self.addCleanup(self.temp.cleanup)
        self.base=Path(self.temp.name);self.root=self.base/'source';self.root.mkdir()
        self.output=self.base/'output'
        for name in SOURCES:
            source=SOURCE_ROOT/'crates/server/src/db'/name
            target=self.root/'crates/server/src/db'/name
            target.parent.mkdir(parents=True,exist_ok=True);target.write_bytes(source.read_bytes())
        for name in ['tokengo-server','tokengo']:
            target=self.root/'target/x86_64-unknown-linux-gnu/release'/name
            target.parent.mkdir(parents=True,exist_ok=True)
            # Header-only contract fixture; no executable is invoked by these tests.
            target.write_bytes(b'\x7fELF\x02\x01'+b'\0'*12+b'\x3e\x00')
        for path,body in {
            'Cargo.lock':b'synthetic lockfile',
            'crates/web/dist/index.html':b'<script src="fixture.js"></script>',
            'crates/web/dist/fixture.js':b'// synthetic',
            'crates/web/dist/fixture.wasm':b'\0asm\x01\0\0\0',
            'deploy/fixture.txt':b'no deployment executed',
            'docs/deployment.md':b'synthetic deployment documentation',
            'docs/operations-tokens.md':b'synthetic operations documentation',
            'docs/account-login.md':b'synthetic account documentation',
            'scripts/mineru-gateway-client.py':b'# synthetic optional tool',
            'LICENSE':b'synthetic license fixture',
            'NOTICE':b'synthetic notice fixture',
        }.items():
            target=self.root/path;target.parent.mkdir(parents=True,exist_ok=True);target.write_bytes(body)

    def build_package(self):
        def git(args,**kwargs):
            self.assertEqual(Path(kwargs['cwd']),self.root)
            if args==['git','status','--porcelain','--untracked-files=normal']:return b''
            if args==['git','rev-parse','HEAD']:return COMMIT+'\n'
            raise AssertionError('unexpected external command: '+repr(args))
        with mock.patch.object(package,'ROOT',self.root),mock.patch.object(package.platform,'system',return_value='Linux'),mock.patch.object(package.platform,'machine',return_value='x86_64'),mock.patch.object(package.subprocess,'check_output',side_effect=git),mock.patch.object(package.sys,'argv',['package.py',str(self.output)]),contextlib.redirect_stdout(io.StringIO()):
            package.main()
        archives=list(self.output.glob('*.tar.gz'));self.assertEqual(len(archives),1)
        return archives[0]

    def unpack_verified(self,archive):
        destination=self.base/'unpacked';destination.mkdir()
        root,commit=manage.verify_bundle(archive,destination)
        self.assertEqual(commit,COMMIT)
        return root,json.loads((root/'manifest.json').read_text())

    def test_actual_archive_manifest_migration45_matches_schema_rs_and_all_checksums(self):
        archive=self.build_package();root,manifest=self.unpack_verified(archive)
        schema=(SOURCE_ROOT/'crates/server/src/db/schema.rs').read_text()
        latest=max(map(int,re.findall(r'^\s*\((\d+),',schema,re.MULTILINE)))
        self.assertEqual(manifest['schema_version'],latest)
        self.assertEqual(latest,45,'a new migration requires a deliberate packaging-contract update')
        self.assertEqual(manifest['commit'],COMMIT)
        self.assertEqual(manifest['target'],'x86_64-unknown-linux-gnu')
        self.assertEqual(set(manifest['migrations']),{'43','44','45'})
        for version,name in [('43','request_paths.sql'),('44','account_overview.sql')]:
            record=manifest['migrations'][version]
            expected=(self.root/'crates/server/src/db'/name).read_bytes()
            self.assertEqual(record['source_path'],'crates/server/src/db/'+name)
            self.assertEqual((root/record['archive_path']).read_bytes(),expected)
            self.assertEqual(record['sha256'],hashlib.sha256(expected).hexdigest())
        executed=re.search(r'\(45,\s*concat!\(include_str!\("([^"]+)"\),\s*"\\n",\s*include_str!\("([^"]+)"\)\)\)',schema)
        self.assertIsNotNone(executed,'migration45 SQL composition changed; review package provenance')
        names=list(executed.groups())
        migration=manifest['migrations']['45']
        self.assertEqual([Path(part['source_path']).name for part in migration['parts']],names)
        raw_parts=[]
        for part in migration['parts']:
            raw=(self.root/part['source_path']).read_bytes();raw_parts.append(raw)
            self.assertEqual((root/part['archive_path']).read_bytes(),raw)
            self.assertEqual(part['sha256'],hashlib.sha256(raw).hexdigest())
        combined=b'\n'.join(raw_parts)
        self.assertEqual((root/migration['archive_path']).read_bytes(),combined)
        self.assertEqual(migration['sha256'],hashlib.sha256(combined).hexdigest())
        checksums=json.loads((root/'checksums.json').read_text())
        for record in [manifest['migrations']['43'],manifest['migrations']['44'],migration,*migration['parts']]:
            self.assertEqual(checksums[record['archive_path']],record['sha256'])
        checksum_line=archive.with_name(archive.name+'.sha256').read_text().strip()
        self.assertEqual(checksum_line,hashlib.sha256(archive.read_bytes()).hexdigest()+'  '+archive.name)
        self.assertIn('docs/deployment.md',checksums)
        self.assertIn('bin/tokengo',checksums)
        self.assertIn('web/fixture.wasm',checksums)

    def test_missing_either_migration45_part_refuses_distributable_archive(self):
        for name in ['query_performance.sql','model_invocation_totals.sql']:
            with self.subTest(missing=name):
                path=self.root/'crates/server/src/db'/name;raw=path.read_bytes();path.unlink()
                try:
                    with self.assertRaises(FileNotFoundError):self.build_package()
                    self.assertEqual(list(self.output.glob('*.tar.gz')),[])
                    self.assertEqual(list(self.output.glob('*.sha256')),[])
                finally:path.write_bytes(raw)

    def test_migration45_combination_preserves_line_endings_and_exact_extra_newline(self):
        first=b'-- source one\r\nSELECT 1;\r\n'
        second='-- source two 中文\nSELECT 2;'.encode()
        (self.root/'crates/server/src/db/query_performance.sql').write_bytes(first)
        (self.root/'crates/server/src/db/model_invocation_totals.sql').write_bytes(second)
        root,manifest=self.unpack_verified(self.build_package())
        migration=manifest['migrations']['45']
        expected=b'-- source one\r\nSELECT 1;\r\n\n'+second
        self.assertEqual((root/migration['archive_path']).read_bytes(),expected)
        self.assertEqual(migration['sha256'],hashlib.sha256(expected).hexdigest())
        self.assertEqual((root/migration['parts'][0]['archive_path']).read_bytes(),first)
        self.assertEqual((root/migration['parts'][1]['archive_path']).read_bytes(),second)


if __name__=='__main__':unittest.main()
