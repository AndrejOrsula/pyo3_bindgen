use super::{
    AttributeVariant, Function, FunctionType, Ident, MethodType, Path, Property, PropertyOwner,
};
use crate::{Config, Result};
use itertools::Itertools;
use rustc_hash::FxHashMap as HashMap;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Class {
    pub name: Path,
    // subclasses: Vec<Class>,
    methods: Vec<Function>,
    properties: Vec<Property>,
    docstring: Option<String>,
}

impl Class {
    pub fn parse(cfg: &Config, class: &pyo3::types::PyType, name: Path) -> Result<Self> {
        let py = class.py();

        // Initialize lists for all members of the class
        // let mut subclasses = Vec::new();
        let mut methods = Vec::new();
        let mut properties = Vec::new();

        // Extract the list of all attribute names in the module
        class
            .dir()
            .iter()
            // Convert each attribute name to an identifier
            .map(|attr_name| Ident::from_py(&attr_name.to_string()))
            .unique()
            // TODO: Try to first access the attribute via __dict__ because Python's descriptor protocol might change the attributes obtained via getattr()
            //       - For example, classmethod and staticmethod are converted to method/function
            //       - However, this might also change some of the parsing and it would need to be fixed
            // Expand each attribute to a tuple of (attr, attr_name, attr_module, attr_type)
            .filter_map(|attr_name| {
                if let Ok(attr) = class.getattr(attr_name.as_py()) {

                    let attr_module = Path::from_py(
                        &attr
                        .getattr(pyo3::intern!(py, "__module__"))
                        .map(std::string::ToString::to_string)
                        .unwrap_or_default(),
                    );
                    let attr_type = attr.get_type();

                    Some((attr, attr_name, attr_module, attr_type))
                } else {
                    eprintln!(
                        "WARN: Cannot get attribute '{attr_name}' of '{name}' even though it is listed in its `__dir__`. Bindings will not be generated.",
                    );
                    None
                }
            })
            // Filter attributes based on various configurable conditions
            .filter(|(_attr, attr_name, attr_module, attr_type)| {
                cfg.is_attr_allowed(attr_name, attr_module, attr_type)
                    || ["__init__", "__call__"].contains(&attr_name.as_py())
            })
            // Iterate over the remaining attributes and parse them
            .try_for_each(|(attr, attr_name, attr_module, attr_type)| {
                let attr_name_full = name.join(&attr_name.clone().into());
                match AttributeVariant::determine(py, attr, attr_type, &attr_module, &name, false)
                    ?
                {
                    AttributeVariant::Import => {
                        eprintln!("WARN: Imports in classes are not supported: '{name}.{attr_name}'. Bindings will not be generated.");
                    }
                    AttributeVariant::Module => {
                        eprintln!(
                            "WARN: Submodules in classes are not supported: '{name}.{attr_name}'. Bindings will not be generated.",
                        );
                    }
                    AttributeVariant::Class => {
                        // let subclass =
                        //     Self::parse(cfg, attr.downcast()?, attr_name_full)?;
                        // subclasses.push(subclass);
                        eprintln!(
                            "WARN: Subclasses in classes are not supported: '{name}.{attr_name}'. Bindings will not be generated.",
                        );
                    }
                    AttributeVariant::Function | AttributeVariant::Method => {
                        let method = Function::parse(
                            cfg,
                            attr,
                            attr_name_full,
                            FunctionType::Method {
                                class_path: name.clone(),
                                typ: match attr_name.as_py() {
                                    "__init__" => MethodType::Constructor,
                                    "__call__" => MethodType::Callable,
                                    _ => MethodType::Unknown,
                                },
                            },
                        )
                        ?;
                        methods.push(method);
                    }
                    AttributeVariant::Closure => {
                        eprintln!("WARN: Closures are not supported in classes: '{attr_name}'. Bindings will not be generated.");
                    }
                    AttributeVariant::TypeVar => {
                        eprintln!("WARN: TypesVars are not supported in classes: '{attr_name}'. Bindings will not be generated.");
                    }
                    AttributeVariant::Property => {
                        let property = Property::parse(
                            cfg,
                            attr,
                            attr_name_full,
                            PropertyOwner::Class,
                        )
                        ?;
                        properties.push(property);
                    }
                }
                Result::Ok(())
            })?;

        // Extract the docstring of the class
        let docstring = {
            let docstring = class.getattr(pyo3::intern!(py, "__doc__"))?.to_string();
            if docstring.is_empty() || docstring == "None" {
                None
            } else {
                Some(docstring)
            }
        };

        Ok(Self {
            name,
            // subclasses,
            methods,
            properties,
            docstring,
        })
    }

    pub fn generate(
        &self,
        cfg: &Config,
        local_types: &HashMap<Path, Path>,
    ) -> Result<proc_macro2::TokenStream> {
        let mut output = proc_macro2::TokenStream::new();

        // Documentation
        if cfg.generate_docs {
            if let Some(mut docstring) = self.docstring.clone() {
                crate::utils::text::format_docstring(&mut docstring);
                output.extend(quote::quote! {
                    #[doc = #docstring]
                });
            }
        }

        // Generate the struct
        let struct_ident: syn::Ident = {
            let name = self.name.name();
            if let Ok(ident) = name.try_into() {
                ident
            } else {
                // Sanitize the struct name
                let new_name = Ident::from_py(&format!(
                    "s_{}",
                    name.as_py().replace(|c: char| !c.is_alphanumeric(), "_")
                ));
                if let Ok(sanitized_ident) = new_name.clone().try_into() {
                    eprintln!(
                        "WARN: Struct '{}' is an invalid Rust ident for a struct name. Renamed to '{}'.",
                        self.name, self.name.parent().unwrap_or_default().join(&new_name.into())
                    );
                    sanitized_ident
                } else {
                    eprintln!(
                        "WARN: Struct '{}' is an invalid Rust ident for a struct name. Renaming failed. Bindings will not be generated.",
                        self.name
                    );
                    return Ok(proc_macro2::TokenStream::new());
                }
            }
        };
        output.extend(quote::quote! {
            #[repr(transparent)]
            pub struct #struct_ident(::pyo3::PyAny);
        });

        // Employ pyo3 macros for native types
        // Note: Using these macros is probably not the best idea, but it makes possible wrapping around ::pyo3::PyAny instead of ::pyo3::PyObject, which improves usability
        let object_name = self.name.to_py();
        output.extend(quote::quote! {
            ::pyo3::pyobject_native_type_named!(#struct_ident);
            ::pyo3::pyobject_native_type_info!(#struct_ident, ::pyo3::pyobject_native_static_type_object!(::pyo3::ffi::PyBaseObject_Type), ::std::option::Option::Some(#object_name));
            ::pyo3::pyobject_native_type_extract!(#struct_ident);
        });

        // Get the names of all methods to avoid name clashes
        let mut scoped_function_idents = self
            .methods
            .iter()
            .map(|method| method.name.name())
            .collect::<Vec<_>>();

        // Generate the struct implementation block
        let mut struct_impl = proc_macro2::TokenStream::new();
        // Methods
        struct_impl.extend(
            self.methods
                .iter()
                .map(|method| method.generate(cfg, &scoped_function_idents, local_types))
                .collect::<Result<proc_macro2::TokenStream>>()?,
        );
        // Properties
        {
            let mut scoped_function_idents_extra = Vec::with_capacity(2);
            if self.methods.iter().any(|method| {
                matches!(
                    method.typ,
                    FunctionType::Method {
                        typ: MethodType::Constructor,
                        ..
                    }
                )
            }) {
                scoped_function_idents_extra.push(Ident::from_py("new"));
            }
            if self.methods.iter().any(|method| {
                matches!(
                    method.typ,
                    FunctionType::Method {
                        typ: MethodType::Callable,
                        ..
                    }
                )
            }) {
                scoped_function_idents_extra.push(Ident::from_py("call"));
            }
            scoped_function_idents.extend(scoped_function_idents_extra.iter());
            struct_impl.extend(
                self.properties
                    .iter()
                    .map(|property| property.generate(cfg, &scoped_function_idents, local_types))
                    .collect::<Result<proc_macro2::TokenStream>>()?,
            );
        }

        // Finalize the implementation block of the struct
        output.extend(quote::quote! {
            #[automatically_derived]
            impl #struct_ident {
                #struct_impl
            }
        });

        Ok(output)
    }
}
