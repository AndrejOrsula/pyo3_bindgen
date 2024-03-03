criterion::criterion_group!(benches, criterion_benchmark);
criterion::criterion_main!(benches);

fn criterion_benchmark(crit: &mut criterion::Criterion) {
    bench_from_str(crit);
    bench_mod(crit);
}

fn bench_from_str(crit: &mut criterion::Criterion) {
    let mut group_from_str = crit.benchmark_group("bindgen_str");
    group_from_str
        .warm_up_time(std::time::Duration::from_millis(250))
        .sample_size(100);

    macro_rules! bench_impl {
        {
            |$criterion:ident|                     $(,)?
            $bench_name:ident                      $(,)?
            $(py)?$(python)?$(:)? $code_py:literal $(,)?
        } => {
            {
                const CODE_PY: &str = indoc::indoc! { $code_py };
                $criterion.bench_function(stringify!($bench_name), |b| {
                    b.iter(|| {
                        pyo3_bindgen_engine::Codegen::default()
                        .module_from_str(
                            criterion::black_box(CODE_PY),
                            criterion::black_box(concat!("bench_mod_", stringify!($bench_name)))
                        )
                        .unwrap()
                        .generate()
                        .unwrap()
                    });
                });
            }
        };
    }

    bench_impl! {
        |group_from_str|
        attribute
        r#"
            t_const_float: float = 0.42
        "#
    }
    bench_impl! {
        |group_from_str|
        function
        r#"
            def t_fn(t_arg1: str) -> int:
                """t_docs"""
                ...
        "#
    }
    bench_impl! {
        |group_from_str|
        class
        r#"
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
}

fn bench_mod(crit: &mut criterion::Criterion) {
    let mut group_module = crit.benchmark_group("bindgen_mod");
    group_module
        .warm_up_time(std::time::Duration::from_secs(2))
        .sample_size(10);

    macro_rules! bench_impl {
        (
            |$criterion:ident|               $(,)?
            $(module:)? $module_name:literal $(,)?
        ) => {
            $criterion.bench_function($module_name, |b| {
                b.iter(|| {
                    pyo3_bindgen_engine::Codegen::default()
                    .module_name(
                        criterion::black_box($module_name)
                    )
                    .unwrap()
                    .generate()
                    .unwrap()
                });
            });
        };
        {
            |$criterion:ident|                          $(,)?
            $(modules:)? [ $($module:literal),+ $(,)? ] $(,)?
        } => {
            $(
                bench_impl!(|$criterion| $module);
            )+
        };
    }

    bench_impl! {
        |group_module|
        modules: [
            "io",
            "math",
            "os",
            "re",
            "sys",
            "time",
        ]
    }

    group_module.finish();
}
