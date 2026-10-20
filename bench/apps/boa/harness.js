// Runs each benchmark of the V8 suite a fixed number of times. base.js's own runner repeats
// each benchmark for as long as a second lasts, so a faster allocator would do more work; a
// fixed count makes every run do the same work. Each benchmark checks its own results and
// throws when they are wrong, which fails the run.
//
// `counts` maps a benchmark's name to how many times it runs. Returns "name:count,..." in
// the order run, for the driver to check that every benchmark ran.
function RunFixed(counts) {
  var done = [];
  var suites = BenchmarkSuite.suites;
  for (var i = 0; i < suites.length; i++) {
    var benchmarks = suites[i].benchmarks;
    for (var j = 0; j < benchmarks.length; j++) {
      var b = benchmarks[j];
      var n = counts[b.name];
      if (n === undefined) {
        throw new Error("no count for benchmark " + b.name);
      }
      b.Setup();
      for (var k = 0; k < n; k++) {
        b.run();
      }
      b.TearDown();
      done.push(b.name + ":" + n);
    }
  }
  return done.join(",");
}
