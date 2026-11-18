"""Post-hoc breakdown/breakout retest analysis on the Exness M15 backtest.

Run from `gold-signal-bot/rust-backend`:
    python3 ../scripts/research/retest_analysis.py

This is research only. It does not change the live signal path.
"""

import json, datetime, math, statistics

CACHE='data/backtest/XAUUSDm_M15.jsonl'
REPORT='data/backtest/exness_confirm_directional.json'
COST_R=0.02

def ts(s):
    return int(datetime.datetime.fromisoformat(s.replace('Z','+00:00')).timestamp()*1000)

candles=[json.loads(l) for l in open(CACHE) if l.strip()]
candles.sort(key=lambda c:c['timestamp'])
idx={c['timestamp']:i for i,c in enumerate(candles)}

d=json.load(open(REPORT))
trades=d['variants']['quantum_v3_htf']['trades']

def simulate_retest(trade, wait_bars=8, buffer=3.0, tp_r=(2.0,3.0), max_hold=96):
    direction=trade['direction']
    trig=trade['trigger_direction']
    if trig not in ('BELOW','ABOVE'):
        return None
    entry=trade['entry']
    i=idx.get(ts(trade['trigger_time']))
    if i is None:
        return None
    risk0=abs(trade['entry']-trade['stop_loss'])
    if risk0<=0: return None
    # wait for retest after trigger candle
    retest_i=None
    for j in range(i+1, min(i+1+wait_bars, len(candles))):
        c=candles[j]
        if direction=='SELL':
            if c['high'] >= entry and c['close'] < entry and c['close'] < c['open']:
                retest_i=j; break
            # moved too far without retest
            if c['low'] < entry - 1.0*risk0:
                return None
        else:
            if c['low'] <= entry and c['close'] > entry and c['close'] > c['open']:
                retest_i=j; break
            if c['high'] > entry + 1.0*risk0:
                return None
    if retest_i is None:
        return None
    c=candles[retest_i]
    if direction=='SELL':
        sl=max(c['high'], entry)+buffer
    else:
        sl=min(c['low'], entry)-buffer
    risk=abs(entry-sl)
    if risk<=0: return None
    tp1=entry-risk*tp_r[0] if direction=='SELL' else entry+risk*tp_r[0]
    tp2=entry-risk*tp_r[1] if direction=='SELL' else entry+risk*tp_r[1]
    # entry at retest level; check from retest candle onward (same candle worst-case SL first)
    for k in range(retest_i, min(retest_i+max_hold+1, len(candles))):
        ck=candles[k]
        if direction=='SELL':
            if ck['high'] >= sl:
                return -1.0 - COST_R
            if ck['low'] <= tp2:
                return tp_r[1] - COST_R
            if ck['low'] <= tp1:
                return tp_r[0] - COST_R
        else:
            if ck['low'] <= sl:
                return -1.0 - COST_R
            if ck['high'] >= tp2:
                return tp_r[1] - COST_R
            if ck['high'] >= tp1:
                return tp_r[0] - COST_R
    ck=candles[min(retest_i+max_hold, len(candles)-1)]
    if direction=='SELL':
        r=(entry-ck['close'])/risk
    else:
        r=(ck['close']-entry)/risk
    return r - COST_R

def stats(vals):
    if not vals: return None
    n=len(vals); wins=sum(1 for x in vals if x>0); gross=sum(x for x in vals if x>0); loss=-sum(x for x in vals if x<0)
    pf=gross/loss if loss>0 else float('inf')
    # max drawdown on cumulative R
    cum=0; peak=0; dd=0
    for x in vals:
        cum+=x; peak=max(peak,cum); dd=max(dd,peak-cum)
    return n, wins/n*100, sum(vals), sum(vals)/n, pf, dd

for wait in [4,8,12]:
    for buffer in [3.0,5.0]:
        vals=[]; skipped=0; by={}
        for t in trades:
            r=simulate_retest(t, wait_bars=wait, buffer=buffer)
            if r is None:
                skipped+=1; continue
            vals.append(r)
            by.setdefault(t['scenario_id'],[]).append(r)
        s=stats(vals)
        print(f"wait={wait} buffer={buffer} retest_trades={s[0] if s else 0} skipped={skipped} win%={s[1]:.1f} totalR={s[2]:+.2f} avgR={s[3]:+.3f} PF={s[4]:.2f} maxDD={s[5]:.2f}")
        for k,v in sorted(by.items()):
            ss=stats(v)
            print(f"   {k}: n={ss[0]} win%={ss[1]:.1f} totalR={ss[2]:+.2f} PF={ss[4]:.2f} maxDD={ss[5]:.2f}")
    print()

# original comparison for directional set
orig=[t['r_multiple']-COST_R for t in trades]
s=stats(orig)
print(f"ORIGINAL quantum_v3_htf: n={s[0]} win%={s[1]:.1f} totalR={s[2]:+.2f} avgR={s[3]:+.3f} PF={s[4]:.2f} maxDD={s[5]:.2f}")
for k in ['bearish_breakdown','bullish_breakout']:
    v=[t['r_multiple']-COST_R for t in trades if t['scenario_id']==k]
    ss=stats(v)
    print(f"   {k}: n={ss[0]} win%={ss[1]:.1f} totalR={ss[2]:+.2f} PF={ss[4]:.2f} maxDD={ss[5]:.2f}")
