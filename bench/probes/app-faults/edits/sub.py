def sub(path, old, new):
    s = open(path).read()
    if s.count(old) != 1:
        raise SystemExit(f"{path}: {s.count(old)} matches for {old[:60]!r}")
    open(path, "w").write(s.replace(old, new))
