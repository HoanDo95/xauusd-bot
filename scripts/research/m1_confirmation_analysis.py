"""Preliminary M1-confirmation analysis on the Exness 100k M1 history.

Run from `gold-signal-bot/rust-backend`:
    python3 ../scripts/research/m1_confirmation_analysis.py

Research only; no live signal change. The sample is ~103 days.
"""

import json, datetime, statistics
M1='data/backtest/XAUUSDm_M1_100k.json'
REPORT='data/backtest/exness_confirm_directional.json'
COST_R=0.02

def ts(s): return int(datetime.datetime.fromisoformat(s.replace('Z','+00:00')).timestamp()*1000)
m1=json.load(open(M1))['bars']
m1.sort(key=lambda b:b['t'])
m1_by={b['t']:b for b in m1}
first_m1=m1[0]['t']*1000; last_m1=m1[-1]['t']*1000
trades=json.load(open(REPORT))['variants']['quantum_v3_htf']['trades']
period=[t for t in trades if first_m1 <= ts(t['trigger_time']) <= last_m1]
print('M1 period', datetime.datetime.utcfromtimestamp(first_m1/1000).isoformat()+'Z', 'to', datetime.datetime.utcfromtimestamp(last_m1/1000).isoformat()+'Z')
print('trades in period',len(period))

def analyze(wait_m1=3, require_direction=True):
    deltas=[]; confirmed=0; skipped=0; r_orig=[]; r_new=[]
    for t in period:
        trig=ts(t['trigger_time'])
        direction=t['direction']; entry=t['entry']; sl=t['stop_loss']; exit_price=t['exit_price']; exit_ts=ts(t['exit_time'])
        fill=None
        for b in m1:
            if b['t']*1000 <= trig: continue
            if b['t']*1000 > trig + wait_m1*60*1000: break
            if require_direction:
                ok=(direction=='SELL' and b['c']<b['o'] and b['c']<entry) or (direction=='BUY' and b['c']>b['o'] and b['c']>entry)
            else:
                ok=(direction=='SELL' and b['c']<entry) or (direction=='BUY' and b['c']>entry)
            if ok:
                fill=b; break
        if fill is None:
            skipped+=1; continue
        fill_ts=fill['t']*1000
        if exit_ts <= fill_ts:
            skipped+=1; continue
        m1_entry=fill['c']
        risk=abs(m1_entry-sl)
        if risk<=0: skipped+=1; continue
        if direction=='SELL': r=(m1_entry-exit_price)/risk
        else: r=(exit_price-m1_entry)/risk
        confirmed+=1
        r_orig.append(t['r_multiple']-COST_R)
        r_new.append(r-COST_R)
        # improvement in entry price: positive = better
        if direction=='SELL': deltas.append((entry-m1_entry)/risk)
        else: deltas.append((m1_entry-entry)/risk)
    def st(v):
        if not v: return None
        n=len(v); wins=sum(1 for x in v if x>0); g=sum(x for x in v if x>0); l=-sum(x for x in v if x<0)
        return n, wins/n*100, sum(v), sum(v)/n, (g/l if l>0 else float('inf'))
    so=st(r_orig); sn=st(r_new)
    print(f"wait={wait_m1}m dir={require_direction}: confirmed={confirmed} skipped={skipped}")
    if so and sn:
        print(f"  original: n={so[0]} win%={so[1]:.1f} totalR={so[2]:+.2f} avgR={so[3]:+.3f} PF={so[4]:.2f}")
        print(f"  m1 entry: n={sn[0]} win%={sn[1]:.1f} totalR={sn[2]:+.2f} avgR={sn[3]:+.3f} PF={sn[4]:.2f}")
        print(f"  entry improvement avg={statistics.mean(deltas):+.3f}R median={statistics.median(deltas):+.3f}R")
for w in [1,3,5,8]:
    analyze(w, True)
for w in [3,5]:
    analyze(w, False)
