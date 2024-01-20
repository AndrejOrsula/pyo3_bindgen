macro_rules! bench_bindgen_from_str {
    {
        |$criterion:ident|                      $(,)?
        $bench_name:ident                       $(,)?
        $(py)?$(python)? $(:)? $code_py:literal $(,)?
    } => {
        {
            const CODE_PY: &str = indoc::indoc! { $code_py };
            $criterion.bench_function(stringify!($bench_name), |b| {
                b.iter(|| {
                    pyo3_bindgen_engine::generate_bindings_from_str(
                        criterion::black_box(CODE_PY),
                        criterion::black_box(concat!("bench_mod_", stringify!($bench_name))),
                    )
                    .unwrap()
                });
            });
        }
    };
}

macro_rules! try_bench_bindgen_for_module {
    {
        |$py:ident, $criterion:ident|         $(,)?
        $(module)? $(:)? $module_name:literal $(,)?
    } => {
        if let Ok(module) = $py.import($module_name) {
            $criterion.bench_function(concat!("bench_bindgen_module_", $module_name), |b| {
                b.iter(|| {
                    pyo3_bindgen_engine::generate_bindings_for_module(
                        criterion::black_box($py),
                        criterion::black_box(module),
                    )
                    .unwrap()
                });
            });
        }
    };
}

fn criterion_benchmark(crit: &mut criterion::Criterion) {
    let mut group_from_str = crit.benchmark_group("generate_bindings_from_str");
    group_from_str
        .warm_up_time(std::time::Duration::from_millis(250))
        .sample_size(100);
    bench_bindgen_from_str! {
        |group_from_str|
        bench_bindgen_attribute
        py: r#"
            t_const_float: float = 0.42
        "#
    }
    bench_bindgen_from_str! {
        |group_from_str|
        bench_bindgen_function
        py: r#"
            def t_fn(t_arg1: str) -> int:
                """t_docs"""
                ...
        "#
    }
    bench_bindgen_from_str! {
        |group_from_str|
        bench_bindgen_class
        py: r#"
            from typing import Dict, Optional
            class t_class:
                """t_docs"""
                def __init__(self, t_arg1: str, t_arg2: Optional[int] = None):
                    """t_docs_init"""
                    ...
                def t_method(self, t_arg1: Dict[str, int], **kwargs):
                    """t_docs_method"""
                    ...
                @property
                def t_prop(self) -> int:
                    ...
                @t_prop.setter
                def t_prop(self, value: int):
                    ...
        "#
    }
    group_from_str.finish();

    let mut group_for_module = crit.benchmark_group("generate_bindings_for_module");
    group_for_module
        .warm_up_time(std::time::Duration::from_secs(2))
        .sample_size(10);
    pyo3::Python::with_gil(|py| {
        try_bench_bindgen_for_module! {
            |py, group_for_module|
            module: "os"
        }
        try_bench_bindgen_for_module! {
            |py, group_for_module|
            module: "sys"
        }
        try_bench_bindgen_for_module! {
            |py, group_for_module|
            module: "numpy"
        }
    });
    group_for_module.finish();
}

criterion::criterion_group!(benches, criterion_benchmark);
criterion::criterion_main!(benches);
