"""Built WASM browser tests with synthetic HTTP responses; no production access.
Run on build-test: browser-venv/bin/python browser-qa.py DIST OUTPUT_DIR
Backend aggregation/authorization must be validated separately by server tests.
"""
import argparse
from datetime import datetime, timedelta
import functools
import http.server
import json
from pathlib import Path
import socket
import threading
from urllib.parse import parse_qs, urlsplit
from zoneinfo import ZoneInfo
from playwright.sync_api import sync_playwright, expect

p = argparse.ArgumentParser()
p.add_argument('dist', type=Path)
p.add_argument('output', type=Path)
a = p.parse_args()
assert socket.gethostname() == 'build-test', 'Browser QA must run on build-test'
assert (a.dist / 'index.html').is_file(), 'Build WASM dist before running'
a.output.mkdir(parents=True, exist_ok=True)
results = []
SUMMARY = dict(request_count=60, ledger_count=65, input_tokens=5000, output_tokens=1000,
               total_tokens=6000, unknown_input_entries=2, unknown_output_entries=3,
               unpriced_entries=1, known_debit_micro=123456)
CATALOG = {kind: [dict(id=1, name='验收对象', user_name='验收用户', state='active')]
           for kind in ['users', 'keys', 'channels']}

class Handler(http.server.SimpleHTTPRequestHandler):
    def translate_path(self, path):
        return super().translate_path(path.removeprefix('/ui'))
    def log_message(self, *_):
        pass

def run_case(browser, origin, role, width, zone, now, full=True):
    context = browser.new_context(viewport=dict(width=width, height=1000), timezone_id=zone)
    epoch = int(now.timestamp() * 1000)
    context.add_init_script(f'''{{const RealDate=Date; globalThis.Date=class extends RealDate {{
      constructor(...args){{super(...(args.length ? args : [{epoch}]));}}
      static now(){{return {epoch};}}
    }};}}''')
    page = context.new_page()
    errors, calls = [], []
    fail = [False]
    page.on('pageerror', lambda e: errors.append(str(e)))
    account = dict(id=1, username='fixture', display_name='验收账号', role=role,
                   user_id=1 if role == 'user' else None, enabled=True, created_at_ms=0)
    ledger_path = '/api/usage' if role == 'root' else '/api/account/portal'
    def route_api(route):
        url = urlsplit(route.request.url)
        if url.path == ledger_path:
            q = {k: v[0] for k, v in parse_qs(url.query).items()}
            calls.append(q)
            if fail[0]:
                route.fulfill(status=500, json={'error': {'message': '合成查询失败'}})
                return
            entry = dict(id=50 if 'before_id' in q else 100, request_id=100, user_id=1,
                         key_id=1, channel_id=1, model='fixture-model', billing_unit='token',
                         status='success', outcome='success', usage_certainty='reported',
                         input_tokens=10, output_tokens=2, debit_micro=123,
                         created_ms=epoch, settled_ms=epoch, record_state='active',
                         user_name='验收用户', key_name='验收 Key', channel_name='验收接入')
            usage = dict(entries=[entry], summary=SUMMARY, next_before_id=None if 'before_id' in q else 100)
            response = usage if role == 'root' else dict(account=account, keys=[], usage=usage)
        else:
            response = {
                '/api/auth/status': dict(needs_setup=False, has_password=True, gateway_key_set=True),
                '/api/account/me': dict(account=account),
                '/api/billing/objects': CATALOG,
                '/api/budgets': dict(budgets=[]),
                '/api/account/keys': dict(keys=[], models=[], services=[], policy={}, budget={}),
            }.get(url.path, {})
        route.fulfill(json=response)
    page.route('**/api/**', route_api)
    page.goto(origin + ('/ui/#billing' if role == 'root' else '/ui/#portal'))
    if role == 'root':
        page.locator('.billing-nav').get_by_role('button', name='用量账本').click()
        scope = page.locator('.billing-usage')
    else:
        scope = page.locator('.portal-page')
    expect(page.locator('#usage-period')).to_have_value('today')
    expect(scope.locator('.portal-summary strong').filter(has_text=('6000' if role == 'root' else '6,000'))).to_have_count(1)
    # Python zoneinfo supplies an independent calendar-boundary oracle.
    midnight = now.replace(hour=0, minute=0, second=0, microsecond=0)
    def check_bounds(start, end):
        assert int(calls[-1]['start_at']) == int(start.timestamp()), calls[-1]
        assert int(calls[-1]['end_at']) == int(end.timestamp()), calls[-1]
    check_bounds(midnight, midnight + timedelta(days=1))
    query = page.get_by_role('button', name='查询', exact=True)
    def submit(button=query):
        with page.expect_response(lambda r: urlsplit(r.url).path == ledger_path):
            button.click()
        expect(query).to_be_enabled()
    def preset(value, start, end):
        page.locator('#usage-period').select_option(value)
        submit()
        check_bounds(start, end)
    monday = midnight - timedelta(days=midnight.weekday())
    preset('week', monday, monday + timedelta(days=7))
    first = midnight.replace(day=1)
    next_month = first.replace(year=first.year + (first.month == 12), month=first.month % 12 + 1)
    preset('month', first, next_month)
    if full:
        page.locator('#usage-period').select_option('custom')
        page.locator('#usage-start').fill('2026-11-01T12:00:01')
        page.locator('#usage-end').fill('2026-11-01T12:00:59')
        submit()
        check_bounds(datetime(2026,11,1,12,0,1,tzinfo=ZoneInfo(zone)), datetime(2026,11,1,12,0,59,tzinfo=ZoneInfo(zone)))
        expect(scope).to_contain_text('12:00:01')
        expect(scope).to_contain_text('12:00:59')
        page.evaluate('document.activeElement.blur(); scrollTo(0,0)')
        assert not page.evaluate('document.documentElement.scrollWidth > innerWidth'), 'Custom filter overflow'
        page.screenshot(path=str(a.output / f'{role}-{width}-custom.png'), full_page=True)
        initial = calls[-1].copy()
        next_button = page.get_by_role('button', name='较早一页' if role == 'root' else '较早记录', exact=True)
        submit(next_button)
        assert {k:v for k,v in calls[-1].items() if k != 'before_id'} == initial
        expect(scope.locator('.portal-summary strong').filter(has_text=('6000' if role == 'root' else '6,000'))).to_have_count(1)
        # Invalid custom order must not issue a request or replace existing data.
        page.locator('#usage-end').fill('2026-10-01T12:00:59')
        count = len(calls)
        query.click()
        expect(scope).to_contain_text('结束时间必须晚于开始时间')
        assert len(calls) == count
        page.locator('#usage-period').select_option('month')
        fail[0] = True
        submit()
        expect(scope).to_contain_text('12:00:01')
        expect(scope.locator('.portal-summary strong').filter(has_text=('6000' if role == 'root' else '6,000'))).to_have_count(1)
        fail[0] = False
        page.locator('#usage-period').select_option('all')
        submit()
        assert 'start_at' not in calls[-1] and 'end_at' not in calls[-1]
        reset = page.get_by_role('button', name='重置筛选' if role == 'root' else '重置为今天', exact=True)
        submit(reset)
        check_bounds(midnight, midnight + timedelta(days=1))
        expect(page.locator('#usage-period')).to_have_value('today')
        page.evaluate('document.activeElement.blur(); scrollTo(0,0)')
        page.screenshot(path=str(a.output / f'{role}-{width}.png'), full_page=True)
        overflow = page.evaluate('document.documentElement.scrollWidth > innerWidth')
        assert not overflow, f'Page overflow: {role} {width}'
    assert not errors, errors
    results.append(dict(role=role, width=width, timezone=zone, now=now.isoformat(),
                        requests=calls, page_errors=errors, status='passed',
                        authority='built WASM with synthetic API responses'))
    context.close()

with http.server.ThreadingHTTPServer(('127.0.0.1', 0), functools.partial(Handler, directory=str(a.dist))) as server, sync_playwright() as pw:
    threading.Thread(target=server.serve_forever, daemon=True).start()
    browser = pw.chromium.launch(executable_path='/srv/cache/playwright/manual-1248/chrome-linux64/chrome', headless=True, args=['--no-sandbox'])
    try:
        origin = f'http://127.0.0.1:{server.server_port}'
        for role in ['root', 'user']:
            for width in [1440, 390]:
                run_case(browser, origin, role, width, 'Asia/Shanghai', datetime(2026,11,13,15,tzinfo=ZoneInfo('Asia/Shanghai')))
        for day in [(2026,3,8), (2026,11,1), (2026,12,31)]:
            run_case(browser, origin, 'root', 1440, 'America/New_York', datetime(*day,12,tzinfo=ZoneInfo('America/New_York')), full=False)
    finally:
        browser.close()
        (a.output / 'browser-results.json').write_text(json.dumps(dict(host=socket.gethostname(), cases=results,
          limitations='Synthetic API fixtures validate actual WASM UI/date conversion; do not establish backend aggregation or authorization.'), ensure_ascii=False, indent=2))
print(f'PASS {len(results)} browser cases')
