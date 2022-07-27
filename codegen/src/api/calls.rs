// Copyright 2019-2022 Parity Technologies (UK) Ltd.
// This file is dual-licensed as Apache-2.0 or GPL-3.0.
// see LICENSE for license details.

use crate::types::{
    CompositeDefFields,
    TypeGenerator,
};
use frame_metadata::{
    v14::RuntimeMetadataV14,
    PalletMetadata,
};
use heck::{
    ToSnakeCase as _,
    ToUpperCamelCase as _,
};
use proc_macro2::TokenStream as TokenStream2;
use proc_macro_error::abort_call_site;
use quote::{
    format_ident,
    quote,
};
use scale_info::form::PortableForm;

/// Generate calls from the provided pallet's metadata.
///
/// The function creates a new module named `calls` under the pallet's module.
/// ```ignore
/// pub mod PalletName {
///     pub mod calls {
///     ...
///     }
/// }
/// ```
///
/// The function generates the calls as rust structs that implement the `subxt::Call` trait
/// to uniquely identify the call's identity when creating the extrinsic.
///
/// ```ignore
/// pub struct CallName {
///      pub call_param: type,
/// }
/// impl ::subxt::Call for CallName {
/// ...
/// }
/// ```
///
/// Calls are extracted from the API and wrapped into the generated `TransactionApi` of
/// each module.
///
/// # Arguments
///
/// - `metadata` - Runtime metadata from which the calls are generated.
/// - `type_gen` - The type generator containing all types defined by metadata.
/// - `pallet` - Pallet metadata from which the calls are generated.
/// - `types_mod_ident` - The ident of the base module that we can use to access the generated types from.
pub fn generate_calls(
    metadata: &RuntimeMetadataV14,
    type_gen: &TypeGenerator,
    pallet: &PalletMetadata<PortableForm>,
    types_mod_ident: &syn::Ident,
) -> TokenStream2 {
    // Early return if the pallet has no calls.
    let call = if let Some(ref calls) = pallet.calls {
        calls
    } else {
        return quote!()
    };

    let mut struct_defs = super::generate_structs_from_variants(
        type_gen,
        call.ty.id(),
        |name| name.to_upper_camel_case().into(),
        "Call",
    );
    let (call_structs, call_fns): (Vec<_>, Vec<_>) = struct_defs
        .iter_mut()
        .map(|(variant_name, struct_def)| {
            let (call_fn_args, call_args): (Vec<_>, Vec<_>) =
                match struct_def.fields {
                    CompositeDefFields::Named(ref named_fields) => {
                        named_fields
                            .iter()
                            .map(|(name, field)| {
                                let fn_arg_type = &field.type_path;
                                let call_arg = if field.is_boxed() {
                                    quote! { #name: ::std::boxed::Box::new(#name) }
                                } else {
                                    quote! { #name }
                                };
                                (quote!( #name: #fn_arg_type ), call_arg)
                            })
                            .unzip()
                    }
                    CompositeDefFields::NoFields => Default::default(),
                    CompositeDefFields::Unnamed(_) =>
                        abort_call_site!(
                            "Call variant for type {} must have all named fields",
                            call.ty.id()
                        )
                };

            let pallet_name = &pallet.name;
            let call_name = &variant_name;
            let struct_name = &struct_def.name;
            let call_hash = subxt_metadata::get_call_hash(metadata, pallet_name, call_name)
                .unwrap_or_else(|_| abort_call_site!("Metadata information for the call {}_{} could not be found", pallet_name, call_name));

            let fn_name = format_ident!("{}", variant_name.to_snake_case());
            // Propagate the documentation just to `TransactionApi` methods, while
            // draining the documentation of inner call structures.
            let docs = struct_def.docs.take();
            // The call structure's documentation was stripped above.
            let call_struct = quote! {
                #struct_def

                impl ::subxt::Call for #struct_name {
                    const PALLET: &'static str = #pallet_name;
                    const FUNCTION: &'static str = #call_name;
                }
            };
            #[cfg(not(feature = "not-metadata-check"))]
            let client_fn = quote! {
                #docs
                pub fn #fn_name(
                    &self,
                    #( #call_fn_args, )*
                ) -> Result<::subxt::SubmittableExtrinsic<'a, T, X, #struct_name, DispatchError, root_mod::Event>, ::subxt::BasicError> {
                    let runtime_call_hash = {
                        let locked_metadata = self.client.metadata();
                        let metadata = locked_metadata.read();
                        Some(metadata.call_hash::<#struct_name>()?)
                    };

                    if runtime_call_hash == [#(#call_hash,)*] {
                        let call = #struct_name { #( #call_args, )* };
                        Ok(::subxt::SubmittableExtrinsic::new(self.client, call))
                    } else {
                        Err(::subxt::MetadataError::IncompatibleMetadata.into())
                    }
                }
            };

            #[cfg(feature = "not-metadata-check")]
            let client_fn = quote! {
                #docs
                pub fn #fn_name(
                    &self,
                    #( #call_fn_args, )*
                ) -> Result<::subxt::SubmittableExtrinsic<'a, T, X, #struct_name, DispatchError, root_mod::Event>, ::subxt::BasicError> {
                    let call = #struct_name { #( #call_args, )* };
                    Ok(::subxt::SubmittableExtrinsic::new(self.client, call))
                }
            };

            (call_struct, client_fn)
        })
        .unzip();

    let call_ty = type_gen.resolve_type(call.ty.id());
    let docs = call_ty.docs();

    quote! {
        #( #[doc = #docs ] )*
        pub mod calls {
            use super::root_mod;
            use super::#types_mod_ident;

            type DispatchError = #types_mod_ident::sp_runtime::DispatchError;

            #( #call_structs )*

            pub struct TransactionApi<'a, T: ::subxt::Config, X> {
                client: &'a ::subxt::Client<T>,
                marker: ::core::marker::PhantomData<X>,
            }

            impl<'a, T, X> TransactionApi<'a, T, X>
            where
                T: ::subxt::Config,
                X: ::subxt::extrinsic::ExtrinsicParams<T>,
            {
                pub fn new(client: &'a ::subxt::Client<T>) -> Self {
                    Self { client, marker: ::core::marker::PhantomData }
                }

                #( #call_fns )*
            }
        }
    }
}
