use super::{FunctionImplementation, ImportMode, Path, TraitMethod};
use crate::{planning::Type, report::DomainDiagnostic, Config, Result};
use pyo3::prelude::*;

#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize)]
pub(crate) struct Property {
    pub(crate) name: Path,
    owner: PropertyOwner,
    has_getter: bool,
    is_mutable: bool,
    annotation: Type,
    setter_annotation: Type,
    docstring: Option<String>,
    setter_docstring: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize)]
pub(crate) enum PropertyOwner {
    Module,
    Class,
}

pub(crate) struct ParsedLiveProperty {
    pub(crate) property: Property,
    pub(crate) used_declared_annotation: bool,
}

impl Property {
    pub(crate) fn resource_shape(&self) -> crate::resource::StructuralResources {
        let (getter_nodes, getter_depth) = self.annotation.resource_shape();
        let (setter_nodes, setter_depth) = self.setter_annotation.resource_shape();
        crate::resource::StructuralResources {
            definitions: 1,
            recursion_depth: getter_depth.max(setter_depth),
            model_nodes: 1 + getter_nodes + setter_nodes,
        }
    }

    /// Python object path resolved when this property is emitted.
    pub(crate) fn import_path(&self) -> Path {
        self.name.parent().unwrap_or_else(|| unreachable!())
    }

    pub(crate) fn is_python_property_object(
        property: &pyo3::Bound<'_, pyo3::types::PyAny>,
    ) -> Result<bool> {
        // `builtins.property` is writable. Use CPython's canonical type object and
        // the C-level subtype check so real property subclasses remain accepted
        // without consulting a replacement object's `__instancecheck__`.
        Ok(unsafe {
            pyo3::ffi::PyObject_TypeCheck(
                property.as_ptr(),
                std::ptr::addr_of_mut!(pyo3::ffi::PyProperty_Type),
            ) != 0
        })
    }

    pub(crate) fn from_live_annotation(name: Path, annotation: Type, is_mutable: bool) -> Self {
        Self {
            name,
            owner: PropertyOwner::Class,
            has_getter: true,
            setter_annotation: annotation.clone(),
            annotation,
            is_mutable,
            docstring: None,
            setter_docstring: None,
        }
    }

    pub(crate) fn apply_live_annotation(
        &mut self,
        annotation: Type,
        allow_instance_assignment: bool,
    ) {
        self.annotation = annotation.clone();
        self.setter_annotation = annotation;
        self.is_mutable |= allow_instance_assignment;
    }

    /// Remove a setter when the owning class's assignment protocol is known to reject
    /// this name. Descriptor/property writability alone is insufficient: Python routes
    /// `obj.name = value` through the class's `__setattr__` before invoking a descriptor.
    pub(crate) fn restrict_live_mutability(&mut self, class_allows_assignment: bool) {
        self.is_mutable &= class_allows_assignment;
    }

    /// Rough byte size this property contributes under the flat model: a base for the
    /// getter (and setter, when mutable) plus the *emitted* docstrings. See
    /// [`super::Function::estimated_flat_size`].
    pub(crate) fn estimated_flat_size(&self, cfg: &Config) -> i64 {
        // A property emits nothing under the flat model when property generation is
        // disabled, so deduplicating it saves nothing.
        if !cfg.generate_properties {
            return 0;
        }
        const GETTER_BASE: i64 = 200;
        const SETTER_BASE: i64 = 200;
        let mut size = if self.has_getter {
            GETTER_BASE + crate::utils::text::emitted_docstring_len(cfg, self.docstring.as_deref())
        } else {
            0
        };
        if self.is_mutable {
            size += SETTER_BASE
                + crate::utils::text::emitted_docstring_len(cfg, self.setter_docstring.as_deref());
        }
        size
    }

    /// Number of inherent forwarders emitted by the deduplicated model.
    ///
    /// Ordinary mutable properties emit a getter and setter. A write-only Python
    /// `property(fget=None, fset=...)` emits only the setter and must therefore count as
    /// one unit rather than inheriting the ordinary mutable-property cost.
    pub(crate) fn forwarder_units(&self) -> usize {
        usize::from(self.has_getter) + usize::from(self.is_mutable)
    }

    pub(crate) fn plan_module_names(&self, name_allocator: &mut super::naming::NameAllocator) {
        let name = self.name.name();
        let Some(ident) = super::naming::rust_ident(name, super::naming::SymbolKind::Function)
        else {
            return;
        };
        if self.has_getter {
            let getter = if name_allocator.claim(&ident) {
                ident
            } else if let Some(ident) = name_allocator.allocate_prefixed("get_", name) {
                ident
            } else {
                return;
            };
            name_allocator.record_resolution(name, &getter);
        }
        if self.is_mutable {
            let _ = name_allocator.allocate_prefixed("set_", name);
        }
    }

    pub(crate) fn parse(
        cfg: &Config,
        property: &pyo3::Bound<pyo3::types::PyAny>,
        name: Path,
        owner: PropertyOwner,
        diagnostics: &mut Vec<DomainDiagnostic>,
    ) -> Result<Self> {
        Ok(
            Self::parse_with_live_annotation(cfg, property, name, owner, None, diagnostics)?
                .property,
        )
    }

    pub(crate) fn parse_with_live_annotation(
        _cfg: &Config,
        property: &pyo3::Bound<pyo3::types::PyAny>,
        name: Path,
        owner: PropertyOwner,
        declared_annotation: Option<&Type>,
        diagnostics: &mut Vec<DomainDiagnostic>,
    ) -> Result<ParsedLiveProperty> {
        fn declared_or_unknown(
            declared_annotation: Option<&Type>,
            used_declared_annotation: &mut bool,
        ) -> Type {
            match declared_annotation {
                Some(annotation) => {
                    *used_declared_annotation = true;
                    annotation.clone()
                }
                None => Type::Unknown,
            }
        }

        let py = property.py();
        let mut used_declared_annotation = false;

        // Extract the type of the property
        let typ = property.get_type();

        // Do not extract the docstring of the property, because it would point to the docstring of the type/class itself, not this property
        let mut docstring = None;

        // Determine the mutability and type of the property
        let (has_getter, is_mutable, annotation, setter_annotation);
        let mut setter_docstring = None;
        match owner {
            PropertyOwner::Module => {
                has_getter = true;
                is_mutable = true;
                annotation = Type::try_from(typ)?;
                setter_annotation = annotation.clone();
                docstring.clone_from(&setter_docstring);
            }
            PropertyOwner::Class => {
                let inspect = crate::runtime_sentinels::inspect_module(py)?;
                let is_python_property = Self::is_python_property_object(property)?;

                if is_python_property {
                    let getter = property.getattr(pyo3::intern!(py, "fget"))?;
                    has_getter = !getter.is_none();
                    // Extract the annotation from the return of the function (if available)
                    if has_getter {
                        let signature_chain =
                            super::Function::signature_annotation_chain(&inspect, &getter, true);
                        let returns_async_handle =
                            super::Function::property_accessor_returns_async_handle(
                                &inspect, &getter,
                            )?;
                        if let Ok(function_signature) =
                            super::Function::inspect_signature(&inspect, &getter)
                        {
                            annotation = if returns_async_handle {
                                Type::PyAny
                            } else {
                                let return_annotation = function_signature
                                    .getattr(pyo3::intern!(py, "return_annotation"))?;
                                if return_annotation
                                    .is(&function_signature.getattr(pyo3::intern!(py, "empty"))?)
                                {
                                    declared_or_unknown(
                                        declared_annotation,
                                        &mut used_declared_annotation,
                                    )
                                } else {
                                    super::Function::parse_signature_annotation_or_unknown(
                                        return_annotation,
                                        signature_chain.as_deref(),
                                        diagnostics,
                                        &name,
                                        format!("Getter annotation of property '{name}'"),
                                    )
                                }
                            };
                        } else {
                            annotation = if returns_async_handle {
                                Type::PyAny
                            } else if let Some(annotation) = declared_annotation {
                                used_declared_annotation = true;
                                annotation.clone()
                            } else {
                                Type::try_from(typ.clone())?
                            };
                        }

                        // Update the docstring if it is empty
                        if docstring.is_none() {
                            docstring = {
                                let docstring =
                                    getter.getattr(pyo3::intern!(py, "__doc__"))?.to_string();
                                if docstring.is_empty() || docstring == "None" {
                                    None
                                } else {
                                    Some(docstring)
                                }
                            };
                        }
                    } else {
                        // A write-only `property(fset=...)` has no readable value.
                        // Retain a declaration only as setter fallback metadata; generation
                        // must not expose a getter that deterministically raises.
                        // Do not mark the declaration used unless the emitted setter below
                        // also needs it.
                        annotation = declared_annotation.cloned().unwrap_or(Type::Unknown);
                    }
                } else {
                    has_getter = true;
                    annotation = if let Some(annotation) = declared_annotation {
                        used_declared_annotation = true;
                        annotation.clone()
                    } else {
                        Type::try_from(typ.clone())?
                    };
                }

                match is_python_property
                    .then(|| property.getattr(pyo3::intern!(py, "fset")))
                    .transpose()?
                {
                    Some(setter) if !setter.is_none() => {
                        if super::Function::property_accessor_returns_async_handle(
                            &inspect, &setter,
                        )? {
                            // Python's descriptor-assignment protocol cannot return the
                            // awaitable created by an async `fset`: `obj.name = value`
                            // decrefs that coroutine immediately and reports successful
                            // assignment even though the setter body never ran. Do not
                            // advertise a Rust setter with those false mutation semantics.
                            is_mutable = false;
                            setter_annotation = Type::Unknown;
                            crate::report::warn(
                                diagnostics,
                                "async_property_setter_skipped",
                                name.to_py(),
                                format!(
                                    "Property '{name}' has an asynchronous setter. Python \
                                     descriptor assignment discards its awaitable result, so no \
                                     Rust setter is generated."
                                ),
                            );
                        } else {
                            is_mutable = true;

                            // Extract the annotation from the parameter of the function (if available)
                            if let Ok(function_signature) =
                                super::Function::inspect_signature(&inspect, &setter)
                            {
                                let signature_chain = super::Function::signature_annotation_chain(
                                    &inspect, &setter, true,
                                );
                                setter_annotation = {
                                    let param = function_signature
                                        .getattr(pyo3::intern!(py, "parameters"))?
                                        .call_method0(pyo3::intern!(py, "values"))?
                                        .try_iter()?
                                        .nth(1)
                                        .transpose()?;
                                    if let Some(param) = param {
                                        let annotation =
                                            param.getattr(pyo3::intern!(py, "annotation"))?;
                                        if annotation
                                            .is(&param.getattr(pyo3::intern!(py, "empty"))?)
                                        {
                                            declared_or_unknown(
                                                declared_annotation,
                                                &mut used_declared_annotation,
                                            )
                                        } else {
                                            super::Function::parse_signature_annotation_or_unknown(
                                                annotation,
                                                signature_chain.as_deref(),
                                                diagnostics,
                                                &name,
                                                format!("Setter annotation of property '{name}'"),
                                            )
                                        }
                                    } else {
                                        declared_or_unknown(
                                            declared_annotation,
                                            &mut used_declared_annotation,
                                        )
                                    }
                                };
                            } else {
                                setter_annotation = declared_or_unknown(
                                    declared_annotation,
                                    &mut used_declared_annotation,
                                );
                            }

                            setter_docstring = {
                                let docstring =
                                    setter.getattr(pyo3::intern!(py, "__doc__"))?.to_string();
                                if docstring.is_empty() || docstring == "None" {
                                    None
                                } else {
                                    Some(docstring)
                                }
                            };

                            if docstring.is_none() {
                                // Update the getter docstring to match setter docstring if it is still empty
                                docstring.clone_from(&setter_docstring);
                            } else if setter_docstring.is_none() {
                                // Otherwise, update the setter docstring to match the getter docstring if it is still empty
                                setter_docstring.clone_from(&docstring);
                            }
                        }
                    }
                    _ => {
                        // Writable Python `__slots__` entries are
                        // `types.MemberDescriptorType`, not `property` objects, and
                        // expose no `fset`. The descriptor type alone is insufficient:
                        // C-extension fields such as `datetime.timedelta.days` and
                        // `slice.start` use the same type but reject assignment. Require
                        // the descriptor's owning class to declare this exact name in
                        // `__slots__` before emitting a setter.
                        is_mutable = is_writable_slot_descriptor(py, property)?;
                        setter_annotation = if is_mutable {
                            annotation.clone()
                        } else {
                            Type::Unknown
                        };
                        setter_docstring = None;
                    }
                }
            }
        }

        Ok(ParsedLiveProperty {
            property: Self {
                name,
                owner,
                has_getter,
                is_mutable,
                annotation,
                setter_annotation,
                docstring,
                setter_docstring,
            },
            used_declared_annotation,
        })
    }

    /// Construct a [`Property`] from the generic IR with zero `pyo3` usage.
    ///
    /// Builds the same struct as [`Self::parse`] but from strings: no
    /// `inspect.signature()`, no `fget`/`fset` probing.
    pub(crate) fn from_ir(
        _cfg: &Config,
        ir: &crate::schema::PropertyIr,
        owner: PropertyOwner,
        local_annotation_types: &super::IrAnnotationTypes,
    ) -> Result<Self> {
        let name = Path::from_py(&ir.name);
        let is_mutable = !ir.readonly;
        let annotation = super::Function::parse_ir_annotation(&ir.py_type, local_annotation_types)?;
        let setter_annotation = super::Function::parse_ir_annotation(
            ir.setter_type.as_deref().unwrap_or(ir.py_type.as_str()),
            local_annotation_types,
        )?;
        let docstring = ir.docstring.clone();
        let setter_docstring = ir.setter_docstring.clone();

        Ok(Self {
            name,
            owner,
            has_getter: true,
            is_mutable,
            annotation,
            setter_annotation,
            docstring,
            setter_docstring,
        })
    }

    /// Whether the getter or setter type references a collection-protocol type that
    /// lowers to the generic `PyCollection<T>` wrapper.
    pub(crate) fn mentions_collection(&self) -> bool {
        self.annotation.mentions_collection() || self.setter_annotation.mentions_collection()
    }

    pub(crate) fn generate(
        &self,
        cfg: &Config,
        name_allocator: &mut super::naming::NameAllocator,
        local_types: &super::LocalTypes,
        import_mode: &ImportMode,
    ) -> Result<FunctionImplementation> {
        // A property whose name cannot be lowered to a valid Rust identifier (even
        // with the fallback prefix) can emit neither a getter nor a setter. Skip the
        // whole property consistently, rather than silently dropping the getter
        // (`generate_getter` returns an empty method) while `generate_setter` hard-
        // errors -- which would abort the entire enclosing binding over one oddly
        // named mutable attribute. `plan_module_names` already skips the same names.
        if super::naming::rust_ident(self.name.name(), super::naming::SymbolKind::Function)
            .is_none()
        {
            return Ok(match self.owner {
                PropertyOwner::Module => {
                    FunctionImplementation::Function(proc_macro2::TokenStream::new())
                }
                PropertyOwner::Class => FunctionImplementation::Method(TraitMethod::empty()),
            });
        }
        Ok(match self.owner {
            PropertyOwner::Module => {
                let mut functions = proc_macro2::TokenStream::new();

                // Getter
                if self.has_getter {
                    let impl_fn = self
                        .generate_getter(cfg, name_allocator, local_types, import_mode)?
                        .impl_fn;
                    functions.extend(impl_fn);
                }

                // Setter (if mutable)
                if self.is_mutable {
                    let impl_fn = self
                        .generate_setter(cfg, name_allocator, local_types, import_mode)?
                        .impl_fn;
                    functions.extend(impl_fn);
                }

                FunctionImplementation::Function(functions)
            }
            PropertyOwner::Class => {
                let mut trait_fn = proc_macro2::TokenStream::new();
                let mut impl_fn = proc_macro2::TokenStream::new();
                let mut forwarders = Vec::new();

                // Getter
                if self.has_getter {
                    let getter =
                        self.generate_getter(cfg, name_allocator, local_types, import_mode)?;
                    trait_fn.extend(getter.trait_fn);
                    impl_fn.extend(getter.impl_fn);
                    forwarders.extend(getter.forwarders);
                }

                // Setter (if mutable)
                if self.is_mutable {
                    let setter =
                        self.generate_setter(cfg, name_allocator, local_types, import_mode)?;
                    trait_fn.extend(setter.trait_fn);
                    impl_fn.extend(setter.impl_fn);
                    forwarders.extend(setter.forwarders);
                }

                FunctionImplementation::Method(TraitMethod {
                    trait_fn,
                    impl_fn,
                    forwarders,
                })
            }
        })
    }

    pub(crate) fn generate_getter(
        &self,
        cfg: &Config,
        name_allocator: &mut super::naming::NameAllocator,
        local_types: &super::LocalTypes,
        import_mode: &ImportMode,
    ) -> Result<TraitMethod> {
        let mut trait_fn = proc_macro2::TokenStream::new();
        let mut impl_fn = proc_macro2::TokenStream::new();
        let mut forwarders = Vec::new();

        // Documentation
        if cfg.generate_docs {
            if let Some(mut docstring) = self.docstring.clone() {
                crate::utils::text::format_docstring(&mut docstring);
                impl_fn.extend(quote::quote! {
                    #[doc = #docstring]
                });
            }
        }

        // Function
        let function_ident: syn::Ident = {
            let name = self.name.name();
            match super::naming::rust_ident(name, super::naming::SymbolKind::Function) {
                Some(ident) if name_allocator.claim(&ident) => ident,
                Some(_) => name_allocator
                    .allocate_prefixed("get_", name)
                    .ok_or_else(|| {
                        syn::Error::new(proc_macro2::Span::call_site(), "invalid property name")
                    })?,
                None => return Ok(TraitMethod::empty()),
            }
        };
        name_allocator.record_resolution(self.name.name(), &function_ident);
        let param_name = self.name.name().as_py();
        let param_type = self.annotation.clone().into_rs_owned(local_types);
        match &self.owner {
            PropertyOwner::Module => {
                let import =
                    import_mode.resolve(&self.name.parent().unwrap_or_else(|| unreachable!()))?;
                impl_fn.extend(quote::quote! {
                    pub fn #function_ident<'py>(
                        py: ::pyo3::marker::Python<'py>,
                    ) -> ::pyo3::PyResult<#param_type> {
                        ::std::result::Result::Ok(::pyo3::types::PyAnyMethods::extract(
                            &::pyo3::types::PyAnyMethods::getattr(#import.as_any(), ::pyo3::intern!(py, #param_name))?
                        )?)
                    }
                });
            }
            PropertyOwner::Class => {
                let param_name = self.name.name().as_py();
                // Deduplicated model (external-IR or live participant): emit the getter as
                // a *default-bodied* trait method reaching the handle generically via the
                // synthetic root accessor (`Self` is generic in a default body); leave the
                // per-type impl empty.
                let uses_dedup_model = import_mode.uses_dedup_model();
                let obj_access = if uses_dedup_model {
                    quote::quote! { self.__pyo3_obj().bind(py) }
                } else {
                    quote::quote! { self.bind(py) }
                };
                let method = quote::quote! {
                    fn #function_ident<'py>(
                        &self,
                        py: ::pyo3::marker::Python<'py>,
                    ) -> ::pyo3::PyResult<#param_type> {
                        ::std::result::Result::Ok(::pyo3::types::PyAnyMethods::extract(
                            &::pyo3::types::PyAnyMethods::getattr(#obj_access, ::pyo3::intern!(py, #param_name))?
                        )?)
                    }
                };
                if uses_dedup_model {
                    // Move the (already-emitted) docstring into the trait, then append
                    // the default-bodied method. The newtype gets an inherent forwarder
                    // to it (a no-argument getter), so the name resolves unambiguously.
                    trait_fn.extend(std::mem::take(&mut impl_fn));
                    trait_fn.extend(method);
                    forwarders.push(super::Forwarder {
                        ident: function_ident.clone(),
                        params: proc_macro2::TokenStream::new(),
                        args: proc_macro2::TokenStream::new(),
                        ret: param_type.clone(),
                    });
                } else {
                    trait_fn.extend(quote::quote! {
                        fn #function_ident<'py>(
                            &self,
                            py: ::pyo3::marker::Python<'py>,
                        ) -> ::pyo3::PyResult<#param_type>;
                    });
                    impl_fn.extend(method);
                }
            }
        }

        Ok(TraitMethod {
            trait_fn,
            impl_fn,
            forwarders,
        })
    }

    pub(crate) fn generate_setter(
        &self,
        cfg: &Config,
        name_allocator: &mut super::naming::NameAllocator,
        local_types: &super::LocalTypes,
        import_mode: &ImportMode,
    ) -> Result<TraitMethod> {
        let mut trait_fn = proc_macro2::TokenStream::new();
        let mut impl_fn = proc_macro2::TokenStream::new();
        let mut forwarders = Vec::new();

        // Documentation
        if cfg.generate_docs {
            if let Some(mut docstring) = self.setter_docstring.clone() {
                crate::utils::text::format_docstring(&mut docstring);
                impl_fn.extend(quote::quote! {
                    #[doc = #docstring]
                });
            }
        }

        // Function
        let function_ident = name_allocator
            .allocate_prefixed("set_", self.name.name())
            .ok_or_else(|| {
                syn::Error::new(proc_macro2::Span::call_site(), "invalid property name")
            })?;
        let param_name = self.name.name().as_py();
        // Use the setter's own annotation (the live path derives it from `fset`; the IR
        // path from `setter_type`, falling back to the getter type when absent) so the
        // accepted value type matches the property's actual setter, not the getter.
        let param_preprocessing = self.setter_annotation.preprocess_borrowed(
            &syn::Ident::new("p_value", proc_macro2::Span::call_site()),
            local_types,
            // A property setter value is never the synthetic `**kwargs` parameter.
            false,
        );
        let param_type = self.setter_annotation.clone().into_rs_borrowed(local_types);
        match &self.owner {
            PropertyOwner::Module => {
                let import =
                    import_mode.resolve(&self.name.parent().unwrap_or_else(|| unreachable!()))?;
                impl_fn.extend(quote::quote! {
                    pub fn #function_ident<'py>(
                        py: ::pyo3::marker::Python<'py>,
                        p_value: #param_type,
                    ) -> ::pyo3::PyResult<()> {
                        #param_preprocessing
                        ::pyo3::types::PyAnyMethods::setattr(#import.as_any(), ::pyo3::intern!(py, #param_name), p_value)
                    }
                });
            }
            PropertyOwner::Class => {
                // Deduplicated model (external-IR or live participant): emit the setter as
                // a *default-bodied* trait method (see the getter above); leave the
                // per-type impl empty.
                let uses_dedup_model = import_mode.uses_dedup_model();
                let obj_access = if uses_dedup_model {
                    quote::quote! { self.__pyo3_obj().bind(py) }
                } else {
                    quote::quote! { self.bind(py) }
                };
                let method = quote::quote! {
                    fn #function_ident<'py>(
                        &self,
                        py: ::pyo3::marker::Python<'py>,
                        p_value: #param_type,
                    ) -> ::pyo3::PyResult<()> {
                        #param_preprocessing
                        ::pyo3::types::PyAnyMethods::setattr(#obj_access, ::pyo3::intern!(py, #param_name), p_value)
                    }
                };
                if uses_dedup_model {
                    trait_fn.extend(std::mem::take(&mut impl_fn));
                    trait_fn.extend(method);
                    forwarders.push(super::Forwarder {
                        ident: function_ident.clone(),
                        params: quote::quote! { p_value: #param_type },
                        args: quote::quote! { p_value },
                        ret: quote::quote! { () },
                    });
                } else {
                    trait_fn.extend(quote::quote! {
                        fn #function_ident<'py>(
                            &self,
                            py: ::pyo3::marker::Python<'py>,
                            p_value: #param_type,
                        ) -> ::pyo3::PyResult<()>;
                    });
                    impl_fn.extend(method);
                }
            }
        }

        Ok(TraitMethod {
            trait_fn,
            impl_fn,
            forwarders,
        })
    }
}

fn is_writable_slot_descriptor(py: Python<'_>, value: &Bound<'_, PyAny>) -> Result<bool> {
    // `types.MemberDescriptorType` is writable; use the canonical C type while
    // retaining subclasses through `PyObject_TypeCheck`.
    if unsafe {
        pyo3::ffi::PyObject_TypeCheck(
            value.as_ptr(),
            std::ptr::addr_of_mut!(pyo3::ffi::PyMemberDescr_Type),
        ) == 0
    } {
        return Ok(false);
    }

    // Treat incomplete or unusual descriptor metadata conservatively. A false
    // negative only omits an ergonomic setter; a false positive emits a method that
    // deterministically fails at runtime.
    let Ok(owner) = value.getattr(pyo3::intern!(py, "__objclass__")) else {
        return Ok(false);
    };
    let Ok(owner_dict) = owner.getattr(pyo3::intern!(py, "__dict__")) else {
        return Ok(false);
    };
    let Ok(slots) = owner_dict.get_item(pyo3::intern!(py, "__slots__")) else {
        return Ok(false);
    };
    let Ok(descriptor_name) = value
        .getattr(pyo3::intern!(py, "__name__"))
        .and_then(|name| name.extract::<String>())
    else {
        return Ok(false);
    };
    let Ok(owner_name) = owner
        .getattr(pyo3::intern!(py, "__name__"))
        .and_then(|name| name.extract::<String>())
    else {
        return Ok(false);
    };

    let is_declared_slot = |slot_name: &str| {
        mangle_slot_name(slot_name, &owner_name) == descriptor_name
            && owner_dict
                .get_item(descriptor_name.as_str())
                .is_ok_and(|declared| declared.is(value))
    };
    if let Ok(slot_name) = slots.extract::<String>() {
        return Ok(is_declared_slot(&slot_name));
    }
    let Ok(slot_names) = slots.try_iter() else {
        return Ok(false);
    };
    for slot_name in slot_names {
        let Ok(slot_name) = slot_name.and_then(|slot_name| slot_name.extract::<String>()) else {
            return Ok(false);
        };
        if is_declared_slot(&slot_name) {
            return Ok(true);
        }
    }
    Ok(false)
}

fn mangle_slot_name(slot_name: &str, owner_name: &str) -> String {
    if slot_name.starts_with("__") && !slot_name.ends_with("__") {
        let owner_name = owner_name.trim_start_matches('_');
        if !owner_name.is_empty() {
            return format!("_{owner_name}{slot_name}");
        }
    }
    slot_name.to_owned()
}
