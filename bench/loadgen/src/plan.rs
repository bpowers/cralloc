//! The load plan: a list of timed segments, and the schedule of when each request is due.

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Kind {
    /// Every idle connection sends at once; latency is measured from the send.
    Closed,
    /// Requests due at this many per second in total, whether or not the server keeps up.
    Rate(f64),
    /// Nothing is sent; connections stay open.
    Off,
    /// Connections are closed once their last responses arrive; nothing is sent after.
    Close,
}

#[derive(Clone, Debug)]
pub struct Segment {
    pub name: String,
    pub kind: Kind,
    pub begin_ns: u64,
    pub end_ns: u64,
}

impl Segment {
    /// Segments named with a leading `_` (warm-ups) are run but left out of the summary.
    pub fn recorded(&self) -> bool {
        !self.name.starts_with('_')
    }
}

fn parse_rate(s: &str) -> Option<f64> {
    let (num, mul) = match s.strip_suffix(['k', 'K']) {
        Some(n) => (n, 1e3),
        None => (s, 1.0),
    };
    num.parse::<f64>().ok().map(|v| v * mul).filter(|v| *v > 0.0)
}

/// Parses `NAME:KIND:SECS,...`, where KIND is `closed`, `off`, `close` or a rate in requests
/// per second (`120000` or `120k`).
pub fn parse(text: &str) -> Result<Vec<Segment>, String> {
    let mut segs = Vec::new();
    let mut t = 0u64;
    for part in text.split(',').map(str::trim).filter(|p| !p.is_empty()) {
        let f: Vec<&str> = part.split(':').collect();
        let [name, kind, secs] = f[..] else { return Err(format!("segment `{part}` is not NAME:KIND:SECS")) };
        let kind = match kind {
            "closed" => Kind::Closed,
            "off" => Kind::Off,
            "close" => Kind::Close,
            r => Kind::Rate(parse_rate(r).ok_or_else(|| format!("bad kind or rate `{r}` in `{part}`"))?),
        };
        let secs: f64 = secs.parse().map_err(|_| format!("bad duration in `{part}`"))?;
        if secs.is_nan() || secs <= 0.0 {
            return Err(format!("duration must be positive in `{part}`"));
        }
        let len = (secs * 1e9) as u64;
        segs.push(Segment { name: name.to_string(), kind, begin_ns: t, end_ns: t + len });
        t += len;
    }
    if segs.is_empty() {
        return Err("empty plan".into());
    }
    if let Some(i) = segs.iter().position(|s| s.kind == Kind::Close)
        && segs[i..].iter().any(|s| !matches!(s.kind, Kind::Off | Kind::Close))
    {
        return Err("nothing but `off` may follow `close`".into());
    }
    if segs.len() > u16::MAX as usize {
        return Err("too many segments".into());
    }
    Ok(segs)
}

/// The segment in force at `t`, if the plan has not ended.
pub fn at(segs: &[Segment], t: u64) -> Option<usize> {
    segs.iter().position(|s| t >= s.begin_ns && t < s.end_ns)
}

/// How arrivals are spaced within a rate segment.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Spacing {
    /// A fixed schedule: one request every `threads / rate` seconds on each thread, the
    /// threads offset so that the whole stream is evenly spaced.
    Uniform,
    /// Exponential gaps with the same mean, from a seeded generator.
    Poisson,
}

/// One thread's due times across every rate segment of the plan, in order.
pub struct Arrivals<'a> {
    segs: &'a [Segment],
    seg: usize,
    next: Option<u64>,
    threads: f64,
    thread: f64,
    spacing: Spacing,
    rng: u64,
}

impl<'a> Arrivals<'a> {
    pub fn new(segs: &'a [Segment], threads: usize, thread: usize, spacing: Spacing, seed: u64) -> Self {
        let mut a = Arrivals {
            segs,
            seg: 0,
            next: None,
            threads: threads as f64,
            thread: thread as f64,
            spacing,
            rng: (seed ^ (thread as u64 + 1).wrapping_mul(0x9e37_79b9_7f4a_7c15)) | 1,
        };
        a.next = a.first_from(0);
        a
    }

    fn uniform01(&mut self) -> f64 {
        self.rng ^= self.rng << 13;
        self.rng ^= self.rng >> 7;
        self.rng ^= self.rng << 17;
        ((self.rng >> 11) as f64 + 0.5) / (1u64 << 53) as f64
    }

    fn gap(&mut self, rate: f64) -> f64 {
        let mean = self.threads / rate * 1e9;
        match self.spacing {
            Spacing::Uniform => mean,
            Spacing::Poisson => -self.uniform01().ln() * mean,
        }
    }

    /// The first due time in segment `from` or a later rate segment.
    fn first_from(&mut self, from: usize) -> Option<u64> {
        for i in from..self.segs.len() {
            if let Kind::Rate(r) = self.segs[i].kind {
                let offset = match self.spacing {
                    Spacing::Uniform => self.thread / r * 1e9,
                    Spacing::Poisson => self.gap(r),
                };
                let due = self.segs[i].begin_ns + offset as u64;
                if due < self.segs[i].end_ns {
                    self.seg = i;
                    return Some(due);
                }
            }
        }
        None
    }

    /// The next request's due time and segment, without consuming it.
    pub fn peek(&self) -> Option<(u64, usize)> {
        self.next.map(|t| (t, self.seg))
    }

    pub fn advance(&mut self) {
        let Some(t) = self.next else { return };
        let Kind::Rate(r) = self.segs[self.seg].kind else { unreachable!() };
        let due = t as f64 + self.gap(r);
        self.next =
            if (due as u64) < self.segs[self.seg].end_ns { Some(due as u64) } else { self.first_from(self.seg + 1) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_times() {
        let p = parse("_w:closed:1, s:10k:2,off:off:0.5,end:close:1").unwrap();
        assert_eq!(p.len(), 4);
        assert!(!p[0].recorded() && p[1].recorded());
        assert_eq!(p[1].kind, Kind::Rate(10_000.0));
        assert_eq!((p[1].begin_ns, p[1].end_ns), (1_000_000_000, 3_000_000_000));
        assert_eq!(at(&p, 3_400_000_000), Some(2));
        assert_eq!(at(&p, 4_500_000_000), None);
        assert!(parse("a:close:1,b:1k:1").is_err());
        assert!(parse("a:x:1").is_err());
        assert!(parse("a:1k").is_err());
    }

    #[test]
    fn uniform_threads_interleave() {
        let p = parse("a:1000:1,b:off:1,c:2000:0.5").unwrap();
        let mut all = Vec::new();
        for t in 0..4 {
            let mut a = Arrivals::new(&p, 4, t, Spacing::Uniform, 1);
            while let Some((due, seg)) = a.peek() {
                all.push((due, seg));
                a.advance();
            }
        }
        all.sort();
        assert_eq!(all.iter().filter(|x| x.1 == 0).count(), 1000);
        assert_eq!(all.iter().filter(|x| x.1 == 2).count(), 1000);
        assert!(all.iter().all(|x| x.1 != 1));
        // Evenly spaced across threads: 1 ms apart in the first segment.
        for w in all[..1000].windows(2) {
            let d = w[1].0 - w[0].0;
            assert!((999_000..=1_001_000).contains(&d), "{d}");
        }
    }

    #[test]
    fn poisson_mean() {
        let p = parse("a:50k:2").unwrap();
        let mut n = 0;
        for t in 0..2 {
            let mut a = Arrivals::new(&p, 2, t, Spacing::Poisson, 7);
            while a.peek().is_some() {
                n += 1;
                a.advance();
            }
        }
        assert!((98_000..102_000).contains(&n), "{n}");
    }
}
