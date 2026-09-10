//! Source-less prelude contracts use the same operation signatures as HIR.

use super::{SnapshotInput, invalid};
use crate::hir::{HirPreludeTraitMethod, HirSerializationTraitMethod, prelude_trait_arity};
use crate::meta::{
    MetaDeclaration, MetaDeclarationKind, MetaGenericParameter, MetaOperation, MetaOrigin,
    MetaVisibility,
};
use crate::meta_frontend::DeriveFrontendError;
use crate::meta_type::MetaTypeRef;
use crate::package::SymbolIdentity;
use crate::resolve::SymbolId;
use std::collections::{BTreeMap, BTreeSet};

impl SnapshotInput<'_> {
    pub(super) fn prelude_declaration(
        &self,
        name: &str,
        identities: &BTreeMap<&SymbolIdentity, SymbolId>,
        pending: &mut Vec<SymbolId>,
        pending_builtins: &mut BTreeSet<String>,
    ) -> Result<MetaDeclaration, DeriveFrontendError> {
        let arity = prelude_trait_arity(name).ok_or_else(|| invalid("unknown prelude trait"))?;
        let methods = HirPreludeTraitMethod::for_trait(name)
            .ok_or_else(|| invalid("prelude trait has no operation catalog"))?;
        let parameters: &[&str] = match name {
            "Iterator" | "AsyncIterator" => &["Element"],
            "Call" | "CallMut" | "CallOnce" => &["Signature"],
            "Encode" | "Decode" => &["Codec"],
            "Encoder" | "Decoder" => &["Codec", "E"],
            _ => &[],
        };
        if arity != parameters.len() {
            return Err(invalid("prelude model arity differs from HIR"));
        }
        let generics = parameters
            .iter()
            .map(|name| {
                MetaGenericParameter::new(*name, Vec::<String>::new())
                    .map_err(|error| invalid(error.to_string()))
            })
            .collect::<Result<Vec<_>, _>>()?;
        let mut names = parameters
            .iter()
            .map(|name| (*name).to_owned())
            .collect::<Vec<_>>();
        names.push("Self".into());
        // Signatures may intern derived shapes. The original HIR is immutable.
        let mut interner = self.hir.interner().clone();
        let mut arguments = (0..names.len())
            .map(|index| {
                interner
                    .generic_parameter(index as u32)
                    .map_err(|error| invalid(error.to_string()))
            })
            .collect::<Result<Vec<_>, _>>()?;
        let mut method_parameters = Vec::new();
        if matches!(name, "Encode" | "Decode") {
            let protocol = if name == "Encode" {
                "Encoder"
            } else {
                "Decoder"
            };
            names.extend(["E".into(), "Adapter".into()]);
            for index in arguments.len()..names.len() {
                arguments.push(
                    interner
                        .generic_parameter(index as u32)
                        .map_err(|error| invalid(error.to_string()))?,
                );
            }
            method_parameters.push(
                MetaGenericParameter::new("E", Vec::<String>::new())
                    .map_err(|error| invalid(error.to_string()))?,
            );
            method_parameters.push(
                MetaGenericParameter::new("Adapter", [format!("{protocol}[$0, $2]")])
                    .map_err(|error| invalid(error.to_string()))?,
            );
            pending_builtins.insert(protocol.into());
        }
        let mut operations = Vec::new();
        let mut types = Vec::new();
        for (ordinal, method) in methods.iter().enumerate() {
            // For codec operations the convention is Codec, E, Self. For
            // Encode/Decode it is Codec, Self, E, Adapter; names above match it.
            let ty = method
                .function_type(&mut interner, &arguments)
                .map_err(|error| invalid(error.to_string()))?
                .ok_or_else(|| invalid("prelude model arguments differ from HIR"))?;
            types.push(ty);
            let signature = MetaTypeRef::resolved_in(
                &interner,
                self.resolved,
                self.packages,
                self.owner,
                ty,
                interner
                    .canonical_interface(ty)
                    .map_err(|error| invalid(error.to_string()))?,
                &names,
            )
            .map_err(|error| invalid(error.to_string()))?;
            let local = if matches!(
                method,
                HirPreludeTraitMethod::Serialization(
                    HirSerializationTraitMethod::Encode | HirSerializationTraitMethod::Decode
                )
            ) {
                method_parameters.clone()
            } else {
                Vec::new()
            };
            operations.push(
                MetaOperation::new(
                    method.method_name(),
                    signature,
                    MetaVisibility::Public,
                    ordinal as u32,
                    MetaOrigin::Builtin(format!("prelude:{name}.{}", method.method_name())),
                    None::<String>,
                )
                .map_err(|error| invalid(error.to_string()))?
                .with_contract(local, false, false),
            );
        }
        self.enqueue_types(&interner, types, identities, pending, pending_builtins)?;
        MetaDeclaration::new(
            name,
            "prelude",
            MetaVisibility::Public,
            generics,
            [],
            MetaOrigin::Builtin(format!("prelude:{name}")),
            None::<String>,
            MetaDeclarationKind::Trait(operations),
        )
        .map_err(|error| invalid(error.to_string()))
    }
}
