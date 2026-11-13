import test from 'node:test';
import assert from 'node:assert/strict';
import {
  checkPanicBaseline,
  countPanicsInSource,
  isTestOrGeneratedPath,
  stripNonCode,
} from './check-panics.mjs';

test('excludes test, benchmark, build, and generated paths while keeping library sources',()=>{
  assert.equal(isTestOrGeneratedPath('maplibre/src/vector/process_vector.rs'),false);
  assert.equal(isTestOrGeneratedPath('maplibre/src/vector/tests.rs'),true);
  assert.equal(isTestOrGeneratedPath('maplibre/src/vector/tests/unreadable.rs'),true);
  assert.equal(isTestOrGeneratedPath('maplibre/tests/expression_conformance.rs'),true);
  assert.equal(isTestOrGeneratedPath('benchmarks/benches/render.rs'),true);
  assert.equal(isTestOrGeneratedPath('web/build.rs'),true);
});

test('ignores comments, strings, raw strings, char literals, and #[cfg(test)] items while preserving lifetimes',()=>{
  const sample=`
    pub fn parse<'a>(input: &'a str) -> &'a str {
      // input.unwrap() inside a line comment
      /* nested /* block.expect("x") */ comment */
      let _lit = "hello .unwrap() \\" .expect(\\"y\\")";
      let _raw = r#"raw panic!("not real") and todo!()"#;
      let _ch = '\\'';
      let _ok = Some(input).unwrap();
      let _must = Some(input).expect("present");
      input
    }

    #[cfg(test)]
    mod tests {
      #[test]
      fn allows_test_unwraps() {
        let x: Option<i32> = Some(1);
        assert_eq!(x.unwrap(), 1);
        panic!("ignored in cfg(test)");
      }
    }

    #[cfg(test)]
    mod external_tests;
  `;

  const counts=countPanicsInSource(sample);
  assert.deepEqual(counts,{
    total:2,
    unwrap:1,
    expect:1,
    panic:0,
    todo:0,
    unimplemented:0,
  });
  assert.ok(stripNonCode("fn f<'a>(x: &'a i32) { let _ = x; }").includes("'a"));
});

test('monotonic ratchet permits equal or lower counts and rejects regressions',()=>{
  const baseline={maplibre:{total:10},web:{total:4}};
  checkPanicBaseline({maplibre:{total:10},web:{total:2}},baseline);
  assert.throws(
    ()=>checkPanicBaseline(
      {maplibre:{total:11,unwrap:9,expect:2,panic:0,todo:0,unimplemented:0}},
      baseline
    ),
    /maplibre: 10 -> 11/
  );
});
