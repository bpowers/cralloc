"""Every page a process was charged for, by structure (docs/research/memory.md, "Small heaps").

usage: acct.py [-v] [-t] PREFIX...    (PREFIX is <out>/<job>.<label>.<round>, as acct.sh writes)

Reads PREFIX.acct (bench/alloc/src/memacct.rs: the cgroup's counters, smaps and the anonymous
pages of every mapping at exit) and, for cralloc builds, PREFIX.memstat (memstat.rs: the heap's
regions, metadata, magazine area and slabs, and, at the sample where the program held the most,
each class's objects by use). Prints, per prefix, KiB by structure; -v adds each symbol of the
binary's data, each class's pages and the objects by use; -t prints one table, a column per
prefix, instead.

The pages of the two probes themselves (memacct's buffers, the memstat sampler's) are left out.
The cgroup's `memory.stat` is not used: the kernel flushes it in batches of megabytes, so a small
cgroup reads zeros. Page tables come from the process's VmPTE; what the cgroup charged beyond
user pages and page tables (kernel stacks, slab objects, the per-CPU charge stock of up to 64
pages per CPU) is the remainder.
"""
import bisect
import collections
import re
import subprocess
import sys

PAGE = 4096
KiB = 1024


def parse_acct(path):
    a = dict(cg={}, stat={}, status={}, maps=[], anon=[], zero=[], skip=[])
    in_smaps = False
    cur = None
    for line in open(path, errors='replace'):
        line = line.rstrip('\n')
        if in_smaps:
            if line == 'end-smaps':
                in_smaps = False
                continue
            m = re.match(r'^([0-9a-f]+)-([0-9a-f]+) (\S+) ([0-9a-f]+) \S+ \S+\s*(.*)$', line)
            if m:
                cur = dict(start=int(m[1], 16), end=int(m[2], 16), perm=m[3], off=int(m[4], 16),
                           name=m[5].strip(), rss=0, anon=0)
                a['maps'].append(cur)
                continue
            m = re.match(r'^(Rss|Anonymous):\s+(\d+) kB', line)
            if m and cur is not None:
                cur['rss' if m[1] == 'Rss' else 'anon'] = int(m[2]) * KiB
            continue
        if line == 'smaps':
            in_smaps = True
        elif line.startswith('cgroup '):
            a['cg'] = {k: int(v) for k, v in (kv.split('=') for kv in line.split()[1:])}
        elif line.startswith('stat '):
            k, v = line[5:].split('=', 1)
            a['stat'][k] = int(v)
        elif line.startswith('status '):
            parts = line.split()
            a['status'][parts[1].rstrip(':')] = int(parts[2]) * (KiB if parts[-1] == 'kB' else 1)
        elif line.startswith(('anon ', 'zero ')):
            kind, s, n = line.split()
            a[kind].append((int(s, 16), int(n)))
        elif line.startswith('self '):
            _, s, n = line.split()
            a['skip'].append((int(s, 16), int(n)))
    return a


def parse_memstat(path, label):
    """The `label` dump: classes, where things are, regions; and the sampler's own memory."""
    out = dict(classes={}, regions=[], where={}, top={}, skip=[])
    try:
        lines = open(path, errors='replace').read().splitlines()
    except OSError:
        return None
    for line in lines:
        tag, _, rest = line.partition(' ')
        kv = dict(p.split('=', 1) for p in rest.split() if '=' in p)
        if tag == label:
            out['top'] = kv
        elif tag == f'{label}-class':
            out['classes'][int(kv['size'])] = kv
        elif tag == f'{label}-where':
            out['where'] = {k: int(v, 16) if k.endswith('base') else int(v) for k, v in kv.items()}
        elif tag == f'{label}-region':
            out['regions'].append(dict(index=int(kv['index']), size=int(kv['size']), base=int(kv['base'], 16),
                                       len=int(kv['len']), meta=int(kv['meta'], 16),
                                       meta_len=int(kv['meta_len'])))
        elif tag == 'sampler-mem':
            s, n = rest.split()
            out['skip'].append((int(s, 16), int(n)))
    return out if out['top'] else None


def symbols(exe, base):
    """(start, end, name) of the binary's data symbols at run-time addresses, sorted."""
    try:
        text = subprocess.run(['nm', '-S', '--defined-only', exe], capture_output=True, text=True).stdout
    except OSError:
        return []
    syms = []
    for line in text.splitlines():
        parts = line.split()
        if len(parts) == 4 and parts[2] in 'bBdDrR':
            addr, size = int(parts[0], 16), int(parts[1], 16)
            if size:
                syms.append((base + addr, base + addr + size, parts[3]))
    syms.sort()
    return syms


def short(name):
    """A mangled Rust symbol's last two path components, loosely."""
    parts = re.findall(r'\d+([A-Za-z_][A-Za-z0-9_]*)', name) if name.startswith(('_R', '_ZN')) else []
    parts = [p for p in parts if not re.match(r'^h[0-9a-f]{16}$', p)]
    return '::'.join(parts[-2:]) if parts else name


STACK = 8 << 20


def classify(prefix):
    a = parse_acct(prefix + '.acct')
    ms = parse_memstat(prefix + '.memstat', 'final')
    peak = parse_memstat(prefix + '.memstat', 'peak')
    by = collections.Counter()
    detail = collections.defaultdict(collections.Counter)
    maps = a['maps']
    skip = a['skip'] + (ms['skip'] if ms else [])
    exe_maps = [m for m in maps if m['name'].endswith(('/workload', '/rpc-server'))]
    exe = exe_maps[0]['name'] if exe_maps else None
    base = min((m['start'] - m['off'] for m in exe_maps), default=0)
    syms = symbols(exe, base) if exe else []
    starts = [s[0] for s in syms]
    slabs = ms['where']['slabs_base'] if ms else None
    regions = ms['regions'] if ms else []
    mags = (ms['where']['mags_base'], ms['where']['mags_base'] + ms['where']['mags_committed']) if ms else None
    starts_maps = [m['start'] for m in maps]

    # Page-table pages by the structure of the first resident page that needs each: a PTE page
    # per 2 MiB, a PMD page per 1 GiB, a PUD page per 512 GiB with anything resident.
    tables = collections.Counter()
    seen_tables = set()

    def add(key, sub):
        if own:
            by[key] += PAGE
            detail[key][sub] += PAGE
        else:
            detail['zero-page reads (no memory, but page tables)'][key] += PAGE
        for shift in (21, 30, 39):
            unit = (shift, addr >> shift)
            if unit not in seen_tables:
                seen_tables.add(unit)
                tables[key] += PAGE

    runs = [(s, n, True) for s, n in a['anon']] + [(s, n, False) for s, n in a['zero']]
    for start, count, own in sorted(runs):
        for i in range(count):
            addr = start + i * PAGE
            if any(s <= addr < s + n for s, n in skip):
                continue
            j = bisect.bisect_right(starts_maps, addr) - 1
            m = maps[j] if j >= 0 and maps[j]['end'] > addr else None
            name = m['name'] if m else '?'
            hit = next((r for r in regions if r['base'] <= addr < r['base'] + r['len']), None)
            if hit:
                add('cralloc: object pages', hit['size'])
                continue
            hit = next((r for r in regions if r['meta'] <= addr < r['meta'] + max(r['meta_len'], PAGE)), None)
            if hit:
                add('cralloc: region metadata', hit['size'])
                detail['metadata pages, class:offset KiB'][f"{hit['size']}:{(addr - hit['meta']) >> 10}{'' if own else ' (zero page)'}"] += PAGE
                continue
            if mags and mags[0] <= addr < mags[1]:
                add('cralloc: magazine area', None)
                continue
            if slabs is not None and slabs <= addr < slabs + 8192 * PAGE:
                add('cralloc: slabs (BSS)', (addr - slabs) // PAGE)
                continue
            if slabs is not None and slabs + 8192 * PAGE <= addr < slabs + 8195 * PAGE:
                add('cralloc: C tables after the slabs (BSS)', None)
                continue
            if exe and name == exe:
                k = bisect.bisect_right(starts, addr + PAGE - 1) - 1
                hits = collections.Counter()
                while k >= 0:
                    s, e, sym = syms[k]
                    if s < addr - (1 << 22):
                        break
                    overlap = min(e, addr + PAGE) - max(s, addr)
                    if overlap > 0:
                        hits[sym] += overlap
                    k -= 1
                sym = short(hits.most_common(1)[0][0]) if hits else '(relocations, GOT)'
                add('binary: data and BSS', sym)
            elif name == '[heap]':
                add('glibc heap (brk)', None)
            elif name == '[stack]':
                add('main thread stack', None)
            elif name.startswith('/'):
                add('shared libraries: data', name.split('/')[-1])
            elif m and STACK <= m['end'] - m['start'] <= STACK + 2 * PAGE:
                add('thread stacks (what is left after exit)', None)
            else:
                add('other anonymous (glibc arenas, TLS)', f"{m['start']:x}" if m else '?')
    detail['page tables by structure (estimate)'] = tables
    return a, by, detail, peak


ORDER = [
    'binary: data and BSS', 'cralloc: slabs (BSS)', 'cralloc: C tables after the slabs (BSS)',
    'cralloc: object pages', 'cralloc: region metadata', 'cralloc: magazine area',
    'glibc heap (brk)', 'main thread stack', 'thread stacks (what is left after exit)',
    'shared libraries: data', 'other anonymous (glibc arenas, TLS)',
]


def rows(prefix):
    a, by, detail, peak = classify(prefix)
    cg, stat = a['cg'], a['stat']
    user = sum(by.values())
    pte = a['status'].get('VmPTE', 0)
    out = [(k, by[k]) for k in ORDER + sorted(set(by) - set(ORDER))]
    out += [('user pages, total', user), ('page tables (VmPTE)', pte),
            ('  estimated from the pages above', sum(detail['page tables by structure (estimate)'].values()))]
    kernel = 0
    if stat.get('anon'):
        out += [('memory.stat anon', stat['anon']), ('memory.stat file', stat.get('file', 0))]
        for k in ('pagetables', 'kernel_stack', 'slab', 'percpu', 'sock', 'vmalloc'):
            out.append((f'memory.stat {k}', stat.get(k, 0)))
        kernel = stat['anon'] + stat.get('file', 0) + sum(
            stat.get(k, 0) for k in ('pagetables', 'kernel_stack', 'slab', 'percpu', 'sock', 'vmalloc'))
        out.append(('rest: charged but in no statistic (stock)', cg.get('current', 0) - kernel))
    else:
        out.append(('rest: kernel stacks, slab, charge stock', cg.get('current', 0) - user - pte))
    out += [('cgroup at exit', cg.get('current', 0)), ('cgroup peak', cg.get('peak', 0))]
    return out, detail, peak


def report(prefix, verbose):
    out, detail, peak = rows(prefix)
    print(f'== {prefix.split("/")[-1]} (KiB)')
    for k, v in out:
        if v:
            print(f'  {k:46} {v / KiB:8.0f}')
            if verbose and detail.get(k):
                for sub, b in sorted(detail[k].items(), key=lambda x: -x[1])[:20]:
                    print(f'      {str(sub)[:60]:60} {b / KiB:6.0f}')
    for k in ('page tables by structure (estimate)', 'zero-page reads (no memory, but page tables)',
              'metadata pages, class:offset KiB'):
        if verbose and detail.get(k):
            print(f'  {k}')
            for sub, b in sorted(detail[k].items(), key=lambda x: (-x[1], str(x[0])))[:40]:
                print(f'      {str(sub)[:60]:60} {b / KiB:6.0f}')
    if verbose and peak:
        print('  objects by class at the peak sample, KiB resident: all, live, loaded, spare, depot, free, above;'
              ' metadata; magazines loaded/spare/depot')
        for size, kv in sorted(peak['classes'].items()):
            vals = [int(kv['resident']), int(kv['live'])]
            vals += [int(kv[k].split('/')[0]) for k in ('loaded', 'spare', 'depot', 'free')]
            vals += [int(kv['above']), int(kv.get('meta', 0))]
            print(f'    {size:6} ' + ' '.join(f'{v / KiB:6.0f}' for v in vals) + f"  {kv['magazines']}")
        t = peak['top']
        print(f"    (peak sample at {t['t_ms']} ms, slabs for {t['cpus']} CPU numbers)")


def table(prefixes):
    cols = [rows(p)[0] for p in prefixes]
    keys = [k for k, _ in cols[0]]
    for c in cols[1:]:
        keys += [k for k, _ in c if k not in keys]
    names = [p.split('/')[-1] for p in prefixes]
    print(f'{"KiB":44}' + ''.join(f'{n[-16:]:>17}' for n in names))
    for k in keys:
        vals = [dict(c).get(k, 0) for c in cols]
        if any(vals):
            print(f'{k:44}' + ''.join(f'{v / KiB:17.0f}' for v in vals))


def main():
    args = sys.argv[1:]
    prefixes = [x for x in args if not x.startswith('-')]
    if '-t' in args:
        table(prefixes)
        return
    for p in prefixes:
        report(p, '-v' in args)


if __name__ == '__main__':
    main()
