"""Summarize memstat files. usage: summ.py [-c] [-s] file...   (-c: per-class rows, -s: sample series)"""
import sys

def kv(line):
    parts = line.split()
    return parts[0], dict(p.split('=', 1) for p in parts[1:])

def mib(x):
    return f'{int(x) / 1048576:7.2f}'

args = sys.argv[1:]
classes = '-c' in args
series = '-s' in args
files = [a for a in args if not a.startswith('-')]
keys = ['rss', 'heap', 'live', 'loaded', 'spare', 'depot', 'free', 'above', 'runs', 'meta', 'mags', 'slabs', 'retained']
for f in files:
    name = f.split('/')[-1].replace('.memstat', '')
    print(f'== {name}')
    print('          ' + ' '.join(f'{k:>7}' for k in keys))
    samples = []
    for line in open(f):
        if not line.strip():
            continue
        tag, d = kv(line)
        if tag == 'sample':
            samples.append(d)
        elif tag in ('peak', 'final'):
            print(f'{tag:5} MiB ' + ' '.join(mib(d[k]) for k in keys) + f"  t={d['t_ms']}ms cpus={d['cpus']} empties={d['empty_magazines']}")
        elif tag.endswith('-class') and classes:
            res = int(d['resident'])
            if res >= 64 << 10:
                print(f"  {tag:11} {d['size']:>8} res={mib(res)} live={mib(d['live'])} loaded={mib(d['loaded'].split('/')[0])} spare={mib(d['spare'].split('/')[0])} depot={mib(d['depot'].split('/')[0])} free={mib(d['free'].split('/')[0])} above={mib(d['above'])} carved={mib(d['carved'])} huge={mib(d['huge_part'])} mags={d['magazines']}")
        elif tag.endswith('-cpu') and classes:
            print(f"  {tag} {d['cpu']:>3} loaded={mib(d['loaded'])} spare={mib(d['spare'])}")
    if series:
        step = max(1, len(samples) // 12)
        for d in samples[::step]:
            print(f"  t={int(d['t_ms']):6d} " + ' '.join(mib(d[k]) for k in keys))
