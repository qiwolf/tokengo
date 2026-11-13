import os,subprocess,json,time,socket,sys
from pathlib import Path
assert socket.gethostname()=='build-test'
r=Path('/srv/build/tokengo-usage-dashboard-20261113');s=r/'source';e=r/'evidence'
os.environ.update(CARGO_HOME='/srv/cache/tokengo-cargo',RUSTUP_HOME='/srv/cache/tokengo-rustup',RUSTUP_TOOLCHAIN='1.98.0',CARGO_TARGET_DIR=str(r/'target'),CARGO_PROFILE_RELEASE_LTO='false',CARGO_PROFILE_RELEASE_CODEGEN_UNITS='16',CARGO_BUILD_JOBS='4',CARGO_INCREMENTAL='0',RUST_TEST_THREADS='4')
os.environ['PATH']='/srv/build/tokengo-shared-virtual-20261022/tools:/srv/cache/tokengo-cargo/bin:'+os.environ['PATH']
checks={'owner-performance':(['cargo','test','--locked','--offline','--release','-p','tokengo-server','--test','account_query_performance','--','--ignored','--nocapture'],s),'migration-unit':(['cargo','test','--locked','--offline','--release','-p','tokengo-server','--lib','usage_time'],s),'web-tests':(['cargo','test','--locked','--offline','--release','-p','tokengo-web','--lib'],s),'web-build':(['trunk','build','--release'],s/'crates/web'),'billing-unit':(['cargo','test','--locked','--offline','--release','-p','tokengo-server','--lib','billing'],s),'billing-http':(['cargo','test','--locked','--offline','--release','-p','tokengo-server','--test','billing','--test','panel_accounts','--test','account_query_performance','--test','migrate_only','--test','account_overview'],s)}
for name in sys.argv[1:]:
 cmd,cwd=checks[name];start=time.time()
 with (e/(name+'.log')).open('w') as f:p=subprocess.run(cmd,cwd=cwd,stdout=f,stderr=subprocess.STDOUT)
 info={'host':socket.gethostname(),'command':cmd,'cwd':str(cwd),'exit_code':p.returncode,'seconds':time.time()-start};(e/(name+'.json')).write_text(json.dumps(info,indent=2));print(json.dumps(info),flush=True)
 if p.returncode:sys.exit(p.returncode)
