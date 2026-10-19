#!/usr/bin/env python3
"""Isolated real Codex client / real gateway / deterministic native Responses source.
No real model or user credentials. JSON evidence contains counts and assertions only.
"""
import argparse, contextlib, http.server, json, os, pathlib, socket, sqlite3, subprocess, tempfile, threading, time, urllib.request

def port():
    with socket.socket() as s:s.bind(('127.0.0.1',0));return s.getsockname()[1]

def main():
    ap=argparse.ArgumentParser();ap.add_argument('--server',required=True);ap.add_argument('--codex',required=True);ap.add_argument('--output',required=True);args=ap.parse_args()
    evidence={'kind':'controlled_client_test','real_provider_tested':False,'client':'Codex','rounds':0,'tool_result_seen':False}
    with tempfile.TemporaryDirectory(prefix='tokengo-codex-') as folder:
        tmp=pathlib.Path(folder);work=tmp/'work';work.mkdir();home=tmp/'codex';home.mkdir(mode=0o700)
        seen=[];fail=[]
        class Source(http.server.BaseHTTPRequestHandler):
            def log_message(self,*_):pass
            def do_POST(self):
                body=json.loads(self.rfile.read(int(self.headers.get('Content-Length','0'))));seen.append(body)
                if self.path!='/v1/responses':self.send_error(404);return
                inputs=body.get('input',[]);results=[item for item in inputs if isinstance(item,dict) and item.get('type')=='function_call_output']
                evidence['rounds']+=1
                base={'id':f'resp_smoke_{evidence["rounds"]}','object':'response','created_at':1,'model':'native-source','status':'in_progress','output':[],'error':None,'usage':None}
                events=[('response.created',{'response':base})]
                if not results:
                    tools=body.get('tools',[])
                    shell=next((t for t in tools if t.get('name') in ('shell_command','exec_command','shell')),None)
                    if not shell:fail.append('no_supported_client_shell_tool');self.send_error(400);return
                    name=shell['name']
                    arguments={'command':'printf TOKENGO_TOOL_OK','workdir':str(work)} if name=='shell_command' else {'cmd':'printf TOKENGO_TOOL_OK','workdir':str(work)} if name=='exec_command' else {'command':['/bin/sh','-c','printf TOKENGO_TOOL_OK'],'workdir':str(work)}
                    argtext=json.dumps(arguments,ensure_ascii=False);item={'id':'fc_smoke_1','type':'function_call','call_id':'call_smoke_1','name':name,'arguments':'','status':'in_progress'}
                    events.append(('response.output_item.added',{'output_index':0,'item':item.copy()}))
                    events.append(('response.function_call_arguments.delta',{'item_id':item['id'],'output_index':0,'delta':argtext}))
                    events.append(('response.function_call_arguments.done',{'item_id':item['id'],'output_index':0,'arguments':argtext}))
                    item.update(arguments=argtext,status='completed');events.append(('response.output_item.done',{'output_index':0,'item':item.copy()}));output=[item]
                else:
                    evidence['tool_result_seen']=any('TOKENGO_TOOL_OK' in json.dumps(r.get('output','')) and r.get('call_id')=='call_smoke_1' for r in results)
                    text='TOKENGO_CLIENT_MULTITURN_OK';item={'id':'msg_smoke_1','type':'message','role':'assistant','status':'in_progress','content':[]}
                    events.extend([('response.output_item.added',{'output_index':0,'item':item.copy()}),('response.content_part.added',{'item_id':item['id'],'output_index':0,'content_index':0,'part':{'type':'output_text','text':'','annotations':[]}}),('response.output_text.delta',{'item_id':item['id'],'output_index':0,'content_index':0,'delta':text}),('response.output_text.done',{'item_id':item['id'],'output_index':0,'content_index':0,'text':text})])
                    content={'type':'output_text','text':text,'annotations':[]};events.append(('response.content_part.done',{'item_id':item['id'],'output_index':0,'content_index':0,'part':content}));item.update(status='completed',content=[content]);events.append(('response.output_item.done',{'output_index':0,'item':item.copy()}));output=[item]
                complete={**base,'status':'completed','output':output,'usage':{'input_tokens':12,'output_tokens':4,'total_tokens':16}}
                events.append(('response.completed',{'response':complete}));wire=b''.join(('event: '+name+'\ndata: '+json.dumps({'type':name,'sequence_number':i,**data},ensure_ascii=False)+'\n\n').encode() for i,(name,data) in enumerate(events))
                self.send_response(200);self.send_header('Content-Type','text/event-stream');self.send_header('Content-Length',str(len(wire)));self.end_headers();self.wfile.write(wire);self.wfile.flush()
        source=http.server.ThreadingHTTPServer(('127.0.0.1',0),Source);threading.Thread(target=source.serve_forever,daemon=True).start()
        gateway_port=port();base=f'http://127.0.0.1:{gateway_port}';db=tmp/'test.db';key='tk-CONTROLLED-CODEX-ONLY-NO-REAL-SECRET'
        env={**os.environ,'TOKENGO_LISTEN':f'127.0.0.1:{gateway_port}','TOKENGO_DB':str(db),'TOKENGO_MASTER_KEY_FILE':str(tmp/'master.key'),'TOKENGO_LOG_DIR':str(tmp/'logs')}
        # Exclude inherited model credentials: the provider has one test-only env_key.
        for name in list(env):
            if name in ('OPENAI_API_KEY','ANTHROPIC_API_KEY','ANTHROPIC_AUTH_TOKEN','OPENAI_BASE_URL','TOKENGO_MASTER_KEY'):env.pop(name,None)
        with open(tmp/'server.log','wb') as logs:
            server=subprocess.Popen([str(pathlib.Path(args.server).resolve())],env=env,cwd=tmp,stdout=logs,stderr=logs)
            try:
                for _ in range(100):
                    try:
                        with urllib.request.urlopen(base+'/livez',timeout=.2):break
                    except Exception:time.sleep(.05)
                else:raise RuntimeError('gateway_start_failed')
                def post(path,value,token=None):
                    headers={'Content-Type':'application/json'}
                    if token:headers['Authorization']='Bearer '+token
                    req=urllib.request.Request(base+path,json.dumps(value).encode(),headers=headers,method='POST')
                    with urllib.request.urlopen(req,timeout=10) as r:return json.load(r)
                setup=post('/api/auth/setup',{'password':'controlled-smoke-password','gateway_key':key});token=setup['token']
                post('/api/channels',{'name':'controlled-responses','base_url':f'http://127.0.0.1:{source.server_port}/v1','key':'TEST-SOURCE-ONLY','protocols':{'native':['responses'],'allow_conversion':False},'models':[{'model':'smoke-native','upstream':'native-source'}]},token)
                # Disable autonomous inference during a controlled client test; no unrelated probes.
                conn=sqlite3.connect(db);conn.execute('INSERT OR REPLACE INTO settings(key,value) VALUES(?,?)',('model_routing:smoke-native',json.dumps({'retry_times':0,'probe_enabled':False,'include_in_auto':False})));conn.commit();conn.close()
                class Tap(http.server.BaseHTTPRequestHandler):
                    def log_message(self,*_):pass
                    def do_POST(self):
                        raw=self.rfile.read(int(self.headers.get('Content-Length','0')));value=json.loads(raw)
                        def structure(t):
                            result={k:t[k] for k in ('type','name') if k in t}
                            if isinstance(t.get('tools'),list):result['tools']=[structure(x) for x in t['tools']]
                            return result
                        evidence['tool_structure']=[structure(t) for t in value.get('tools',[])]
                        req=urllib.request.Request(base+self.path,raw,headers={'Content-Type':'application/json','Authorization':'Bearer '+key},method='POST')
                        try:
                            with urllib.request.urlopen(req,timeout=30) as r:
                                data=r.read();status=r.status;ctype=r.headers.get('Content-Type','application/json')
                        except urllib.error.HTTPError as r:data=r.read();status=r.code;ctype=r.headers.get('Content-Type','application/json')
                        self.send_response(status);self.send_header('Content-Type',ctype);self.send_header('Content-Length',str(len(data)));self.end_headers();self.wfile.write(data)
                tap=http.server.ThreadingHTTPServer(('127.0.0.1',0),Tap);threading.Thread(target=tap.serve_forever,daemon=True).start()
                client_base=f'http://127.0.0.1:{tap.server_port}'
                client_env={**env,'CODEX_HOME':str(home),'TOKENGO_TEST_VKEY':key}
                command=[args.codex,'exec','--ignore-user-config','--ignore-rules','--ephemeral','--json','--skip-git-repo-check','--sandbox','workspace-write','-C',str(work),'-c','model_provider="tokengo_smoke"','-c','model="smoke-native"','-c','model_providers.tokengo_smoke.name="Tokengo smoke"','-c',f'model_providers.tokengo_smoke.base_url="{client_base}/v1"','-c','model_providers.tokengo_smoke.env_key="TOKENGO_TEST_VKEY"','-c','model_providers.tokengo_smoke.wire_api="responses"','-c','features.responses_websockets=false','-c','web_search="disabled"','Run the requested harmless local tool, then report its result.']
                result=subprocess.run(command,env=client_env,cwd=work,stdin=subprocess.DEVNULL,stdout=subprocess.PIPE,stderr=subprocess.PIPE,timeout=70)
                tap.shutdown()
                stdout=result.stdout.decode(errors='replace');stderr=result.stderr.decode(errors='replace')
                evidence.update(client_exit=result.returncode,final_marker_seen='TOKENGO_CLIENT_MULTITURN_OK' in stdout,tool_execution_marker_seen='TOKENGO_TOOL_OK' in stdout,client_version=subprocess.check_output([args.codex,'--version'],text=True).strip(),failures=fail)
                # Only store client diagnostic stderr when it cannot contain a secret/body.
                evidence['diagnostic_categories']=[label for label in ('sandbox','approval','stream','error','retry') if label in stderr.lower()]
                conn=sqlite3.connect(db)
                evidence['request_statuses']=[r[0] for r in conn.execute('SELECT status FROM requests ORDER BY id')]
                evidence['attempt_outcomes']=[r[0] for r in conn.execute('SELECT outcome FROM attempts ORDER BY id')]
                evidence['usage_known']=[list(r) for r in conn.execute('SELECT input_tokens,output_tokens,input_tokens_known,output_tokens_known FROM requests ORDER BY id')]
                conn.close()
                evidence['passed']=result.returncode==0 and evidence['tool_result_seen'] and evidence['final_marker_seen'] and evidence['rounds']==2 and not fail and evidence['attempt_outcomes']==['success','success'] and evidence['request_statuses']==['ok','ok']
                if not evidence['passed']:
                    # Test-only credentials, isolated config and deterministic prompt; emit short sanitized diagnostic.
                    evidence['failure_tail']=(stderr+'\n'+stdout)[-3500:].replace(key,'[TEST KEY]')
            finally:server.terminate();server.wait(timeout=10);source.shutdown()
    pathlib.Path(args.output).write_text(json.dumps(evidence,ensure_ascii=False,indent=2)+'\n');print(json.dumps(evidence,ensure_ascii=False,indent=2))
    return 0 if evidence.get('passed') else 1
if __name__=='__main__':raise SystemExit(main())
