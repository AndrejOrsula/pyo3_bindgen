use criterion::{black_box, criterion_group, criterion_main, BatchSize, Criterion};
use pyo3_bindgen_engine::{
    ClassIr, Codegen, FunctionIr, FunctionKindIr, ImportIr, ModuleIr, SchemaIr,
};
use std::time::Duration;

#[path = "../test_support/combined_scale.rs"]
mod combined_scale;

const COMBINED_BENCH_SCALE: usize = 12;

/// Whether the interpreter this bench binary embeds can import `module`.
fn importable(module: &str) -> bool {
    pyo3::Python::initialize();
    pyo3::Python::attach(|py| py.import(module).is_ok())
}

fn empty_module(name: impl Into<String>) -> ModuleIr {
    ModuleIr {
        name: name.into(),
        docstring: None,
        is_package: false,
        classes: vec![],
        enums: vec![],
        functions: vec![],
        properties: vec![],
        submodules: vec![],
        prelude: vec![],
        imports: vec![],
        type_vars: vec![],
    }
}

fn wide_schema(width: usize) -> SchemaIr {
    let mut module = empty_module("scale_wide");
    module.functions = (0..width)
        .map(|index| FunctionIr {
            name: format!("scale_wide.function_{index}"),
            kind: FunctionKindIr::Function,
            params: vec![],
            return_type: "int".to_owned(),
            docstring: None,
        })
        .collect();
    SchemaIr {
        modules: vec![module],
    }
}

fn deep_inheritance_schema(depth: usize) -> SchemaIr {
    let mut module = empty_module("scale_deep");
    module.classes = (0..depth)
        .map(|index| ClassIr {
            name: format!("scale_deep.Class{index}"),
            base_classes: (index > 0)
                .then(|| format!("scale_deep.Class{}", index - 1))
                .into_iter()
                .collect(),
            docstring: None,
            methods: vec![],
            properties: vec![],
        })
        .collect();
    SchemaIr {
        modules: vec![module],
    }
}

fn dense_import_schema(width: usize) -> SchemaIr {
    let modules = (0..width)
        .map(|index| {
            let mut module = empty_module(format!("scale_imports.module_{index}"));
            module.imports = (0..width)
                .filter(|target| *target != index)
                .map(|target| ImportIr {
                    origin: format!("scale_imports.module_{target}"),
                    target: format!("scale_imports.module_{index}.alias_{target}"),
                })
                .collect();
            module
        })
        .collect();
    SchemaIr { modules }
}

fn incremental_codegen(count: usize) -> Codegen {
    (0..count).fold(Codegen::default(), |codegen, index| {
        codegen
            .modules_from_ir(SchemaIr {
                modules: vec![empty_module(format!("scale_calls.module_{index}"))],
            })
            .unwrap()
    })
}

fn adaptive_source(methods: usize) -> String {
    let methods = (0..methods)
        .map(|index| {
            format!(
                "    def method_{index}(self, value: int, text: str) -> int:\n\
                 \x20       \"\"\"Substantial inherited method for adaptive sizing.\"\"\"\n\
                 \x20       return value\n"
            )
        })
        .collect::<String>();
    format!("class Base:\n{methods}\nclass Derived(Base):\n    pass\n")
}

fn scale_benchmarks(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group("scale_external_ir");
    group
        .sample_size(10)
        .warm_up_time(Duration::from_millis(250));
    group.bench_function("wide_1000_definitions", |bench| {
        bench.iter(|| {
            Codegen::default()
                .modules_from_ir(black_box(wide_schema(1_000)))
                .unwrap()
                .generate()
                .unwrap()
        });
    });
    group.bench_function("deep_24_classes", |bench| {
        bench.iter(|| {
            Codegen::default()
                .modules_from_ir(black_box(deep_inheritance_schema(24)))
                .unwrap()
                .generate()
                .unwrap()
        });
    });
    group.bench_function("dense_6x5_imports", |bench| {
        bench.iter(|| {
            Codegen::default()
                .modules_from_ir(black_box(dense_import_schema(6)))
                .unwrap()
                .generate()
                .unwrap()
        });
    });
    group.bench_function("incremental_1000_calls", |bench| {
        bench.iter(|| incremental_codegen(black_box(1_000)).generate().unwrap());
    });
    // Both rows prepare the same larger schema outside the measured iteration. The batched row
    // measures one registration plus validation, normalization, and emission. The incremental row
    // measures each module registration plus the same terminal work.
    let combined_schema = combined_scale::combined_external_schema(COMBINED_BENCH_SCALE);
    group.bench_function("combined_external_batched", |bench| {
        bench.iter_batched(
            || combined_schema.clone(),
            |schema| {
                black_box(
                    Codegen::default()
                        .modules_from_ir(schema)
                        .unwrap()
                        .generate()
                        .unwrap(),
                )
            },
            BatchSize::SmallInput,
        );
    });
    group.bench_function("combined_external_incremental", |bench| {
        bench.iter_batched(
            || combined_schema.clone(),
            |schema| {
                let codegen = schema
                    .modules
                    .into_iter()
                    .fold(Codegen::default(), |codegen, module| {
                        codegen.module_from_ir(module).unwrap()
                    });
                black_box(codegen.generate().unwrap())
            },
            BatchSize::SmallInput,
        );
    });
    group.finish();

    let mut live = criterion.benchmark_group("scale_live");
    live.sample_size(10)
        .warm_up_time(Duration::from_millis(250));
    let adaptive = adaptive_source(40);
    live.bench_function("adaptive_40_inherited_methods", |bench| {
        bench.iter(|| {
            Codegen::default()
                .module_from_str(black_box(&adaptive), "scale_adaptive")
                .unwrap()
                .generate()
                .unwrap()
        });
    });
    // `cargo test --all-targets` runs benches in test mode, so this heavy target must follow the
    // same contract as the compile sweep: skip when the package is absent, and fail closed only
    // when the caller declares it required. Most CI lanes install no third-party Python packages,
    // and an unconditional `numpy` bench turned every one of them red.
    if importable("numpy") {
        live.bench_function("numpy_live", |bench| {
            bench.iter(|| {
                Codegen::default()
                    .module_name(black_box("numpy"))
                    .unwrap()
                    .generate()
                    .unwrap()
            });
        });
    } else {
        assert!(
            std::env::var_os("PYO3_BINDGEN_REQUIRE_HEAVY").is_none(),
            "PYO3_BINDGEN_REQUIRE_HEAVY is set but the numpy live workload cannot import numpy"
        );
        eprintln!("skipping scale_live/numpy_live: numpy is not importable");
    }
    live.finish();
}

criterion_group!(benches, scale_benchmarks);
criterion_main!(benches);
