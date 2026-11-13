import test from 'node:test';
import assert from 'node:assert/strict';
import {
  checkPanicBaseline,
  countPanicsInSource,
  isTestOrGeneratedPath,
  scanWorkspacePanics,
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

test('per-file ratchet enforces ceiling 0 for unlisted files and per-file ceiling for listed files',()=>{
  const baseline={
    crates:{maplibre:{total:5}},
    files:{
      'maplibre/src/coords.rs':2,
      'maplibre/src/vector/resource/buffer_pool.rs':3,
    },
  };

  // Equal or lower per-file counts pass, even if crate total matches.
  checkPanicBaseline(
    {
      crates:{maplibre:{total:4,unwrap:0,expect:4,panic:0,todo:0,unimplemented:0}},
      fileDetails:{
        'maplibre/src/coords.rs':{total:2,unwrap:0,expect:2,panic:0,todo:0,unimplemented:0},
        'maplibre/src/vector/resource/buffer_pool.rs':{total:2,unwrap:0,expect:2,panic:0,todo:0,unimplemented:0},
      },
    },
    baseline
  );

  // Moving a panic into an unlisted file fails even when the crate-level total stays <= baseline!
  assert.throws(
    ()=>checkPanicBaseline(
      {
        crates:{maplibre:{total:5,unwrap:1,expect:4,panic:0,todo:0,unimplemented:0}},
        fileDetails:{
          'maplibre/src/coords.rs':{total:2,unwrap:0,expect:2,panic:0,todo:0,unimplemented:0},
          'maplibre/src/new_clean_module.rs':{total:1,unwrap:1,expect:0,panic:0,todo:0,unimplemented:0},
          'maplibre/src/vector/resource/buffer_pool.rs':{total:2,unwrap:0,expect:2,panic:0,todo:0,unimplemented:0},
        },
      },
      baseline
    ),
    /file maplibre\/src\/new_clean_module\.rs: 0 -> 1 \(\+1; unwrap=1, expect=0/
  );

  // Exceeding an existing listed file's ceiling fails even when the crate-level total stays <= baseline!
  assert.throws(
    ()=>checkPanicBaseline(
      {
        crates:{maplibre:{total:5,unwrap:0,expect:5,panic:0,todo:0,unimplemented:0}},
        fileDetails:{
          'maplibre/src/coords.rs':{total:3,unwrap:0,expect:3,panic:0,todo:0,unimplemented:0},
          'maplibre/src/vector/resource/buffer_pool.rs':{total:2,unwrap:0,expect:2,panic:0,todo:0,unimplemented:0},
        },
      },
      baseline
    ),
    /file maplibre\/src\/coords\.rs: 2 -> 3 \(\+1; unwrap=0, expect=3/
  );
});

test('scanWorkspacePanics emits only positive sorted POSIX file entries',()=>{
  const report=scanWorkspacePanics();
  const keys=Object.keys(report.files);
  const sorted=[...keys].sort((a,b)=>(a<b?-1:a>b?1:0));
  assert.deepEqual(keys,sorted);
  for(const [relPath,count] of Object.entries(report.files)){
    assert.ok(!relPath.includes('\\'),`expected POSIX path, got ${relPath}`);
    assert.ok(Number.isInteger(count)&&count>0,`expected positive integer for ${relPath}, got ${count}`);
  }
});
