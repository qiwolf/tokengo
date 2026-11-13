"""Actual WASM currency/unit-price browser checks using synthetic HTTP only.
Run on build-test. Does not validate real settlement, ASR, upstream support or deployment.
"""
import argparse
from copy import deepcopy
from datetime import datetime
import functools
import http.server
import json
from pathlib import Path
import socket
import threading
from urllib.parse import parse_qs,urlsplit
from zoneinfo import ZoneInfo
from playwright.sync_api import sync_playwright,expect

p=argparse.ArgumentParser();p.add_argument('dist',type=Path);p.add_argument('output',type=Path);a=p.parse_args()
assert socket.gethostname()=='build-test';assert (a.dist/'index.html').is_file();a.output.mkdir(parents=True,exist_ok=True)
NOW=datetime(2026,11,13,15,tzinfo=ZoneInfo('Asia/Shanghai'));EPOCH=int(NOW.timestamp()*1000);results=[]
def mapping(model,upstream=None,enabled=True,pricing_enabled=True):return dict(model=model,upstream=upstream or model,enabled=enabled,pricing_enabled=pricing_enabled)
def channel(id,name,models,**kw):return dict(id=id,name=name,provider_id=1,provider_name='验收供应方',enabled=True,provider_enabled=True,base_url=f'https://source-{id}.invalid/v1',models=models,**kw)
CHANNELS=[channel(101,'主渠道',[mapping('ASR-Local','vendor/asr-a'),mapping('ASR-Local','vendor/asr-a'),mapping('Chat-Local','vendor/chat'),mapping('禁用映射',enabled=False),mapping('全局停用模型',enabled=True,pricing_enabled=False)]),
          channel(102,'备用渠道',[mapping('ASR-Local','vendor/asr-b'),mapping('TTS-Local','vendor/tts')]),
          channel(103,'停用渠道',[mapping('停用渠道模型')]),channel(104,'停用供应方渠道',[mapping('停用供应方模型')]),
          channel(105,'停用凭据渠道',[mapping('停用凭据模型')]),channel(106,'过期凭据渠道',[mapping('过期凭据模型')]),
          channel(107,'停用映射渠道',[mapping('只有停用映射',enabled=False)])]
CHANNELS[2]['enabled']=False;CHANNELS[3]['provider_enabled']=False
CATALOG=dict(users=[dict(id=1,name='用户甲',state='active')],keys=[dict(id=11,user_id=1,user_name='用户甲',name='甲主Key',state='active')],
             channels=[dict(id=c['id'],name=c['name'],state='historical' if c['id'] in [103,104,105,106] else 'active') for c in CHANNELS])
CREDENTIALS=[dict(id=i,label=name,enabled=enabled,source_ids=[source],expires_at=(EPOCH-1000 if source==106 else None))
             for i,(source,name,enabled) in enumerate([(101,'主Key',True),(102,'备用Key',True),(105,'停用Key',False),(106,'过期Key',True),(107,'映射Key',True)],1)]
TOKEN_PRICES=[dict(id=10,channel_id=101,model='chat local',currency='CNY',input_micro_per_million=1000000,output_micro_per_million=2000000,max_input_tokens=1000,max_output_tokens=500),
              dict(id=9,channel_id=101,model='chat local',input_micro_per_million=7000000,output_micro_per_million=9000000,max_input_tokens=2000,max_output_tokens=1000)]
REQUEST_PRICES=[dict(id=4,channel_id=101,model='chat local',currency='CNY',micro_per_call=100000),dict(id=3,channel_id=101,model='chat local',currency='USD',micro_per_call=800000)]
UNIT_PRICES=[dict(id=7,channel_id=101,model='asr local',currency='CNY',unit='minute',micro_per_unit=120000,max_quantity_milli=30000),dict(id=8,channel_id=101,model='asr local',currency='CNY',unit='hour',micro_per_unit=540000,max_quantity_milli=1800000),
             dict(id=90,channel_id=101,model='asr local',currency='USD',unit='hour',micro_per_unit=880000,max_quantity_milli=1800000)]

def analytics(q,coin,summary):
    start=int(q.get('start_at',EPOCH//1000-3600));end=int(q.get('end_at',start+86400));step=max(1,(end-start+7)//8)
    buckets=[dict(start_at=start+i*step,request_count=(summary['request_count'] if i==2 else 0),ledger_count=(summary['ledger_count'] if i==2 else 0),
        input_tokens=(summary['input_tokens'] if i==2 else 0),output_tokens=(summary['output_tokens'] if i==2 else 0),total_tokens=(summary['total_tokens'] if i==2 else 0),
        known_debit_micro=(summary['known_debit_micro'] if i==2 else 0),unknown_input_entries=0,unknown_output_entries=0) for i in range(8)]
    model=dict(model='chat local',request_count=summary['request_count'],ledger_count=summary['ledger_count'],input_tokens=summary['input_tokens'],output_tokens=summary['output_tokens'],total_tokens=summary['total_tokens'],known_debit_micro=summary['known_debit_micro'])
    return dict(currency=coin,start_at=start,end_at=end,effective_end_at=min(end,EPOCH//1000),bucket_seconds=step,buckets=buckets,models=[model],average_rpm=0.05,average_tpm=2.5,
                model_series=[dict(model='chat local',is_other=False,ledger_count=summary['ledger_count'],request_count=summary['request_count'],buckets=buckets)])

def entries(coin):
    base=dict(user_id=1,key_id=11,channel_id=101,status='success',outcome='success',created_ms=EPOCH,settled_ms=EPOCH,record_state='active',user_name='用户甲',key_name='甲主Key',channel_name='主渠道',reserved_micro=0)
    if coin=='USD':return [dict(base,id=90,request_id=90,model='legacy-usd',billing_unit='token',usage_certainty='reported',input_tokens=40,output_tokens=10,debit_micro=2500000,observed_cost_micro=2000000)] # legacy missing currency intentionally
    return [dict(base,id=100,request_id=100,currency='CNY',model='chat local',billing_unit='token',usage_certainty='reported',input_tokens=120,output_tokens=30,debit_micro=400000,observed_cost_micro=300000),
            dict(base,id=99,request_id=99,currency='CNY',model='asr local',billing_unit='hour',quantity_milli=61234,usage_certainty='measured',input_tokens=None,output_tokens=None,debit_micro=540000,observed_cost_micro=540000),
            dict(base,id=98,request_id=98,currency='CNY',model='tts local',billing_unit='character',quantity_milli=3000,usage_certainty='measured',input_tokens=None,output_tokens=None,debit_micro=310000,observed_cost_micro=310000)]

class Handler(http.server.SimpleHTTPRequestHandler):
    def translate_path(self,path):return super().translate_path(path.removeprefix('/ui'))
    def log_message(self,*_):pass

def run_case(browser,origin,role,width,theme):
    context=browser.new_context(viewport=dict(width=width,height=1100),timezone_id='Asia/Shanghai',color_scheme=theme)
    context.add_init_script(f'''{{const D=Date;globalThis.Date=class extends D{{constructor(...args){{super(...(args.length?args:[{EPOCH}]));}}static now(){{return {EPOCH};}}}};localStorage.setItem('tokengo.theme','{theme}');}}''')
    page=context.new_page();errors=[];calls=[];posts=[];budgets={};prices={"/api/pricing":deepcopy(TOKEN_PRICES),"/api/request-pricing":deepcopy(REQUEST_PRICES),"/api/unit-pricing":deepcopy(UNIT_PRICES)}
    page.on('pageerror',lambda e:errors.append(str(e)))
    account=dict(id=1,username='fixture',display_name='验收账号',role=role,user_id=1 if role=='user' else None,enabled=True,created_at_ms=0)
    ledger='/api/usage' if role=='root' else '/api/account/portal'
    def route_api(route):
        url=urlsplit(route.request.url);q={k:v[0] for k,v in parse_qs(url.query).items()};method=route.request.method;body=route.request.post_data_json if method in ['POST','PUT'] else None
        calls.append(dict(path=url.path,query=q,method=method,body=body))
        if url.path==ledger:
            coin=q.get('currency','CNY');summary=dict(currency=coin,request_count=3 if coin=='CNY' else 1,ledger_count=3 if coin=='CNY' else 1,
                input_tokens=120 if coin=='CNY' else 40,output_tokens=30 if coin=='CNY' else 10,total_tokens=150 if coin=='CNY' else 50,
                known_debit_micro=1250000 if coin=='CNY' else 2500000,unknown_input_entries=2 if coin=='CNY' else 0,unknown_output_entries=2 if coin=='CNY' else 0,unpriced_entries=0,unknown_debit_entries=0)
            usage=dict(entries=entries(coin),summary=summary,analytics=analytics(q,coin,summary),next_before_id=None)
            response=usage if role=='root' else dict(account=account,keys=[],usage=usage)
        elif url.path in prices:
            array,item={"/api/pricing":('prices','price'),"/api/request-pricing":('request_prices','request_price'),"/api/unit-pricing":('unit_prices','unit_price')}[url.path]
            if method=='POST':
                posts.append(dict(path=url.path,query=q,body=body));row=dict(body,id=100+len(posts));prices[url.path].insert(0,row);response={item:row}
            else:response={array:prices[url.path]}
        elif url.path=='/api/budgets':
            coin=q.get('currency','CNY')
            if coin not in budgets:budgets[coin]=dict(scope='user',scope_id=1,currency=coin,limit_micro=25000000 if coin=='CNY' else 10000000,
                spent_micro=1250000 if coin=='CNY' else 2500000,reserved_micro=540000 if coin=='CNY' else 0,frozen=(coin=='USD'),object_name='用户甲',user_name='用户甲',object_state='active',status_label='有效')
            if method=='PUT':posts.append(dict(path=url.path,query=q,body=body));budgets[coin].update(body)
            response=dict(budgets=[budgets[coin]])
        else:
            response={'/api/auth/status':dict(needs_setup=False,has_password=True,gateway_key_set=True),'/api/account/me':dict(account=account),
                '/api/billing/objects':CATALOG,'/api/channels':dict(channels=CHANNELS),'/api/providers/1/credentials':dict(credentials=CREDENTIALS),
                '/api/account/keys':dict(keys=[],models=[],services=[],policy={},budget={})}.get(url.path,{})
        route.fulfill(json=response)
    page.route('**/api/**',route_api);page.goto(origin+('/ui/#billing' if role=='root' else '/ui/#portal'))
    nav=page.locator('.billing-nav')
    def screenshot(name,target=None):
        page.evaluate('document.activeElement.blur();scrollTo(0,0)')
        assert not page.evaluate('document.documentElement.scrollWidth>innerWidth'),f'overflow {role} {width} {theme} {name}'
        if target is None:page.screenshot(path=str(a.output/f'{role}-{width}-{theme}-{name}.png'),full_page=False)
        else:target.screenshot(path=str(a.output/f'{role}-{width}-{theme}-{name}.png'))
    def response(action,path,method='GET'):
        with page.expect_response(lambda r:urlsplit(r.url).path==path and r.request.method==method):action()
    if role=='root':nav.get_by_role('button',name='用量账本').click()
    scope=page.locator('.billing-usage' if role=='root' else '.portal-page');dash=scope.locator('.usage-dashboard')
    expect(dash.locator('.usage-kpi').nth(1)).to_contain_text('¥1.25');expect(dash.locator('.usage-kpi').nth(1)).to_contain_text('CNY')
    table=scope.locator('table').filter(has=page.locator('tbody')).first
    expect(table).to_contain_text('61.234 秒音频');expect(table).to_contain_text('3 字符')
    if role=='root':expect(table).to_contain_text('已计量');expect(table).to_contain_text('0.54 元')
    else:expect(table).to_contain_text('0.54 CNY')
    dash.locator('.usage-metric-switch').get_by_role('button',name='扣款',exact=True).click();expect(dash.locator('.usage-trend .usage-y-axis')).to_contain_text('¥')
    screenshot('cny-dashboard')
    if role=='root':page.get_by_role('button',name='更多筛选',exact=True).click()
    coin_picker=page.locator('#usage-currency' if role=='root' else '#portal-currency');expect(coin_picker).to_have_value('CNY')
    coin_picker.select_option('USD');expect(dash.locator('.usage-kpi').nth(1)).to_contain_text('¥1.25');response(lambda:page.get_by_role('button',name='查询',exact=True).click(),ledger)
    expect(dash.locator('.usage-kpi').nth(1)).to_contain_text('$2.5');expect(dash.locator('.usage-kpi').nth(1)).to_contain_text('USD')
    expect(table).to_contain_text('2.5 USD');expect(table).not_to_contain_text('元')
    dash.locator('.usage-metric-switch').get_by_role('button',name='扣款',exact=True).click();expect(dash.locator('.usage-trend .usage-y-axis')).to_contain_text('$')
    screenshot('usd-history')
    response(lambda:page.get_by_role('button',name='重置',exact=True).click(),ledger);expect(dash.locator('.usage-kpi').nth(1)).to_contain_text('¥1.25')
    if role=='root':
        # Currency-specific budgets: old USD cap remains read-only; explicit unfreeze preserves exact cap.
        nav.get_by_role('button',name='额度管理').click();expect(page.locator('#budget-currency')).to_have_value('CNY')
        expect(page.locator('.billing-budgets')).to_contain_text('25 元')
        page.get_by_role('button',name='设置额度',exact=True).click();budget=page.locator('#budget-dialog')
        page.locator('#budget-object-user').select_option('1');page.locator('#budget-limit').fill('12.345678')
        response(lambda:budget.get_by_role('button',name='保存额度',exact=True).click(),'/api/budgets','PUT');expect(budget).not_to_be_visible()
        assert posts[-1]==dict(path='/api/budgets',query=dict(scope='user',scope_id='1',currency='CNY'),body=dict(limit_micro=12345678,frozen=False)),posts[-1]
        response(lambda:page.locator('#budget-currency').select_option('USD'),'/api/budgets')
        expect(page.get_by_role('button',name='设置额度',exact=True)).to_be_disabled();expect(page.locator('.billing-budgets')).to_contain_text('10 USD')
        page.get_by_role('button',name='管理额度',exact=True).click();expect(page.locator('#budget-limit')).to_have_attribute('readonly','')
        expect(page.locator('#budget-limit')).to_have_value('10');expect(budget.get_by_role('button',name='解除限额',exact=True)).to_be_disabled()
        budget.get_by_role('checkbox').uncheck();screenshot('usd-budget-readonly',budget)
        response(lambda:budget.get_by_role('button',name='保存额度',exact=True).click(),'/api/budgets','PUT')
        assert posts[-1]==dict(path='/api/budgets',query=dict(scope='user',scope_id='1',currency='USD'),body=dict(limit_micro=10000000,frozen=False)),posts[-1]
        # The model must be chosen before any source. Duplicates and inactive catalog/credentials/mappings are absent.
        nav.get_by_role('button',name='价格配置').click();expect(page.locator('#price-versions')).to_have_value('current')
        expect(page.locator('.billing-price-table')).to_contain_text('1 元');expect(page.locator('.billing-price-table')).not_to_contain_text('USD')
        page.get_by_role('button',name='配置人民币价格',exact=True).click();dialog=page.locator('#price-dialog')
        expect(page.locator('#price-channel')).to_be_disabled()
        expect(page.locator('#price-model option')).to_have_count(4)
        assert page.locator('#price-model option').evaluate_all('(els)=>els.map(e=>e.value)')==['','asr local','chat local','tts local']
        page.locator('#price-model').select_option('asr local');expect(page.locator('#price-channel option')).to_have_count(3)
        page.locator('#price-channel').select_option('101');expect(dialog).to_contain_text('渠道：验收供应方 · 主渠道');expect(dialog).to_contain_text('上游模型：vendor/asr-a');expect(dialog).to_contain_text('API 地址：https://source-101.invalid/v1')
        page.locator('#price-model').select_option('chat local');expect(page.locator('#price-channel')).to_have_value('');expect(page.locator('#price-channel option')).to_have_count(2)
        page.locator('#price-channel').select_option('101');page.locator('#price-in').fill('1.234567');page.locator('#price-out').fill('2.000001');page.locator('#price-max-in').fill('12345');page.locator('#price-max-out').fill('678')
        screenshot('model-first-token-price',dialog)
        response(lambda:dialog.get_by_role('button',name='创建人民币价格版本',exact=True).click(),'/api/pricing','POST');expect(dialog).not_to_be_visible()
        assert posts[-1]['body']==dict(channel_id=101,model='chat local',currency='CNY',input_micro_per_million=1234567,output_micro_per_million=2000001,max_input_tokens=12345,max_output_tokens=678),posts[-1]
        page.locator('#price-versions').select_option('history');legacy=page.locator('.billing-price-table tbody tr').filter(has_text='历史美元')
        expect(legacy).to_contain_text('7 USD');expect(legacy).to_contain_text('9 USD');n=len(posts)
        legacy.get_by_role('button',name='为此模型配置人民币价格',exact=True).click();expect(dialog).to_contain_text('旧美元金额不会复制成人民币')
        expect(page.locator('#price-in')).to_have_value('');expect(page.locator('#price-out')).to_have_value('');assert len(posts)==n
        screenshot('legacy-price-reuse-empty',dialog);dialog.get_by_role('button',name='取消',exact=True).click()
        page.get_by_role('button',name='按请求计费',exact=True).click();page.get_by_role('button',name='配置人民币价格',exact=True).click();dialog=page.locator('#request-price-dialog')
        page.locator('#request-price-model').select_option('chat local');page.locator('#request-price-channel').select_option('101');page.locator('#request-price-in').fill('0.012345')
        response(lambda:dialog.get_by_role('button',name='创建人民币价格版本',exact=True).click(),'/api/request-pricing','POST')
        assert posts[-1]['body']==dict(channel_id=101,model='chat local',currency='CNY',micro_per_call=12345),posts[-1]
        page.get_by_role('button',name='按时长 / 数量计费',exact=True).click()
        # A unit change supersedes the earlier CNY rate for the same source/model.
        # A newer USD version must never suppress the separate current CNY rate.
        expect(page.locator('.billing-price-table tbody tr')).to_have_count(1)
        expect(page.locator('.billing-price-table')).to_contain_text('0.54 元 / 小时')
        expect(page.locator('.billing-price-table')).not_to_contain_text('0.12 元')
        page.locator('#unit-price-versions').select_option('history')
        expect(page.locator('.billing-price-table')).to_contain_text('0.12 元 / 分钟')
        expect(page.locator('.billing-price-table')).to_contain_text('0.88 USD / 小时')
        page.locator('#unit-price-versions').select_option('current')
        for unit,model,source,price,max_qty,expected_price,expected_qty in [('hour','asr local','101','0.54','61.234',540000,61234),('minute','asr local','101','0.06','61.234',60000,61234),('second','asr local','101','0.001','61.234',1000,61234),('character','tts local','102','0.000123','10.5',123,10500),('byte','tts local','102','0.000001','1024',1,1024000),('image','chat local','101','1.25','3',1250000,3000),('video','chat local','101','2.5','2',2500000,2000)]:
            page.get_by_role('button',name='配置人民币价格',exact=True).click();dialog=page.locator('#unit-price-dialog')
            page.locator('#unit-price-model').select_option(model);page.locator('#unit-price-channel').select_option(source);page.locator('#unit-price-unit').select_option(unit)
            page.locator('#unit-price-in').fill(price);page.locator('#unit-price-max').fill(max_qty)
            if unit=='hour':
                expect(dialog).to_contain_text('价格（元 / 小时）');expect(dialog).to_contain_text('单次预占音频时长上限（秒）');expect(dialog).to_contain_text('请求前按上限预占，完成后按实际计量结算');expect(dialog).to_contain_text('对应调用接口尚未接入自动结算');screenshot('asr-hour-price',dialog)
                n=len(posts);page.locator('#unit-price-max').fill('1.2345');dialog.get_by_role('button',name='创建人民币价格版本',exact=True).click();expect(dialog).to_contain_text('最多3位小数');assert len(posts)==n
                page.locator('#unit-price-max').fill(max_qty);page.locator('#unit-price-in').fill('1.0000001');dialog.get_by_role('button',name='创建人民币价格版本',exact=True).click();expect(dialog).to_contain_text('最多6位小数');assert len(posts)==n
                page.locator('#unit-price-in').fill(price)
            response(lambda:dialog.get_by_role('button',name='创建人民币价格版本',exact=True).click(),'/api/unit-pricing','POST');expect(dialog).not_to_be_visible()
            assert posts[-1]['body']==dict(channel_id=int(source),model=model,currency='CNY',unit=unit,micro_per_unit=expected_price,max_quantity_milli=expected_qty),posts[-1]
        expect(page.locator('.billing-price-table tbody tr')).to_have_count(3)
        screenshot('unit-price-versions')
        page.locator('#unit-price-versions').select_option('history');legacy=page.locator('.billing-price-table tbody tr').filter(has_text='历史美元')
        expect(legacy).to_contain_text('0.88 USD');n=len(posts)
        legacy.get_by_role('button',name='为此模型配置人民币价格',exact=True).click();expect(dialog).to_contain_text('旧美元金额不会复制成人民币')
        expect(page.locator('#unit-price-in')).to_have_value('');expect(page.locator('#unit-price-unit')).to_have_value('hour');expect(page.locator('#unit-price-max')).to_have_value('1800');assert len(posts)==n
        screenshot('legacy-unit-price-reuse-empty',dialog);dialog.get_by_role('button',name='取消',exact=True).click()
    assert not errors,errors
    results.append(dict(role=role,width=width,theme=theme,status='passed',api_calls=calls,synthetic_mutations=posts,page_errors=errors,
        authority='Actual built WASM + synthetic HTTP; form payloads only, no real payment or settlement'))
    context.close()

with http.server.ThreadingHTTPServer(('127.0.0.1',0),functools.partial(Handler,directory=str(a.dist))) as server,sync_playwright() as pw:
    threading.Thread(target=server.serve_forever,daemon=True).start();browser=pw.chromium.launch(executable_path='/srv/cache/playwright/manual-1248/chrome-linux64/chrome',headless=True,args=['--no-sandbox'])
    try:
        origin=f'http://127.0.0.1:{server.server_port}'
        for role in ['root','user']:
            for width in [1440,390]:
                for theme in ['light','dark']:run_case(browser,origin,role,width,theme)
    finally:
        browser.close();(a.output/'browser-results.json').write_text(json.dumps(dict(host=socket.gethostname(),cases=results,
          limitations='Synthetic API fixtures validate actual WASM display/forms. Real CNY accounting, ASR media metering, authorization and settlement require separate server tests; no production access.'),ensure_ascii=False,indent=2))
print(f'PASS {len(results)} CNY/unit-pricing browser cases')
