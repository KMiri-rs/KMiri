//! User-facing pretty-printing of interpreter locals.

use rustc_abi::{FieldIdx, Size};
use rustc_middle::mir;
use rustc_middle::ty::{self, Ty};

use crate::debugger::state::LocalKind;
use crate::*;

const MAX_DEPTH: u8 = 3;
const MAX_ITEMS: u64 = 8;
const MAX_STR: usize = 64;

pub fn format_local<'tcx>(
    ecx: &MiriInterpCx<'tcx>,
    frame: &Frame<'tcx, Provenance, FrameExtra<'tcx>>,
    local: mir::Local,
) -> (String, LocalKind) {
    let state = &frame.locals()[local];
    if state.as_mplace_or_imm_ghost().is_none() {
        return ("-".to_owned(), LocalKind::Dead);
    }

    let Some(op) = local_to_op(ecx, frame, local) else {
        return ("uninit".to_owned(), LocalKind::Uninitialized);
    };
    let kind = kind_for_ty(op.layout.ty);
    match format_op(ecx, &op, 0) {
        None => ("uninit".to_owned(), LocalKind::Uninitialized),
        Some(text) => (text, kind),
    }
}

fn local_to_op<'tcx>(
    ecx: &MiriInterpCx<'tcx>,
    frame: &Frame<'tcx, Provenance, FrameExtra<'tcx>>,
    local: mir::Local,
) -> Option<OpTy<'tcx>> {
    let ty = frame.body().local_decls[local].ty;
    let layout = ecx.layout_of(ty).ok()?;
    match frame.locals()[local].as_mplace_or_imm_ghost() {
        None => None,
        Some(Either::Right(imm)) => Some(ImmTy::from_immediate(imm, layout).into()),
        Some(Either::Left((ptr, meta))) =>
            match meta {
                MemPlaceMeta::None => {
                    let mplace = ecx.ptr_to_mplace_unaligned(ptr, layout);
                    mplace.to_op(ecx).discard_err()
                }
                // Wide/unsized locals are stored with metadata; `local_to_op` only
                // reads the current frame, which is the common debugger view.
                MemPlaceMeta::Meta(_) => ecx.local_to_op(local, Some(layout)).discard_err(),
            },
    }
}

fn kind_for_ty(ty: Ty<'_>) -> LocalKind {
    match ty.kind() {
        ty::Ref(..) | ty::RawPtr(..) | ty::FnPtr(..) => LocalKind::Pointer,
        _ => LocalKind::Initialized,
    }
}

fn format_op<'tcx>(ecx: &MiriInterpCx<'tcx>, op: &OpTy<'tcx>, depth: u8) -> Option<String> {
    if depth > MAX_DEPTH {
        return Some("..".to_owned());
    }

    let ty = op.layout.ty;
    if op.layout.is_zst() {
        return Some(format_zst(ty));
    }

    match *ty.kind() {
        ty::Bool => format_bool(ecx, op),
        ty::Char => format_char(ecx, op),
        ty::Int(_) | ty::Uint(_) => format_integer(ecx, op),
        ty::Float(_) => format_float(ecx, op),
        ty::Str => format_str(ecx, op),
        ty::Ref(_, inner, _) => format_ref(ecx, op, inner, depth),
        ty::RawPtr(inner, _) => format_raw_ptr(ecx, op, inner),
        ty::FnPtr(..) => format_fn_ptr(ecx, op),
        ty::FnDef(..) => Some(ty.to_string()),
        ty::Adt(def, _) if def.is_enum() => format_enum(ecx, op, def, depth),
        ty::Adt(def, _) if def.is_union() =>
            Some(format!("<union {}>", ecx.tcx.item_name(def.did()))),
        ty::Adt(def, _) => format_struct(ecx, op, def, depth),
        ty::Tuple(_) => format_tuple(ecx, op, depth),
        ty::Array(_, _) => format_array_like(ecx, op, depth, ('[', ']')),
        ty::Slice(_) => format_array_like(ecx, op, depth, ('[', ']')),
        ty::Never => Some("!".to_owned()),
        ty::Closure(..) =>
            format_tuple(ecx, op, depth).map(|fields| format!("{{closure}}{fields}")),
        _ => format_fallback(ecx, op),
    }
}

fn format_zst(ty: Ty<'_>) -> String {
    if ty.is_unit() {
        "()".to_owned()
    } else if let ty::Adt(..) = ty.kind() {
        ty.to_string().rsplit("::").next().unwrap_or("").to_owned()
    } else {
        ty.to_string()
    }
}

fn format_bool<'tcx>(ecx: &MiriInterpCx<'tcx>, op: &OpTy<'tcx>) -> Option<String> {
    let imm = ecx.read_immediate(op).discard_err()?;
    Some(imm.to_scalar().to_bool().discard_err()?.to_string())
}

fn format_char<'tcx>(ecx: &MiriInterpCx<'tcx>, op: &OpTy<'tcx>) -> Option<String> {
    let imm = ecx.read_immediate(op).discard_err()?;
    let bits = imm.to_scalar().to_u32().discard_err()?;
    match char::from_u32(bits) {
        Some(c) => Some(format!("{c:?}")),
        None => Some(format!("<invalid char 0x{bits:x}>")),
    }
}

fn format_integer<'tcx>(ecx: &MiriInterpCx<'tcx>, op: &OpTy<'tcx>) -> Option<String> {
    let imm = ecx.read_immediate(op).discard_err()?;
    let scalar = imm.to_scalar();
    let size = op.layout.size;
    if op.layout.backend_repr.is_signed() {
        let n = scalar.to_int(size).discard_err()?;
        Some(format_signed(n, size))
    } else {
        let n = scalar.to_uint(size).discard_err()?;
        Some(format_unsigned(n, size))
    }
}

fn format_signed(n: i128, size: Size) -> String {
    if size.bytes() > 1 && n.abs() >= 10 { format!("{n} (0x{n:x})") } else { n.to_string() }
}

fn format_unsigned(n: u128, size: Size) -> String {
    if size.bytes() > 1 && n >= 10 { format!("{n} (0x{n:x})") } else { n.to_string() }
}

fn format_float<'tcx>(ecx: &MiriInterpCx<'tcx>, op: &OpTy<'tcx>) -> Option<String> {
    let imm = ecx.read_immediate(op).discard_err()?;
    let scalar = imm.to_scalar();
    let text = match op.layout.ty.kind() {
        ty::Float(ty::FloatTy::F16) => scalar.to_f16().discard_err()?.to_string(),
        ty::Float(ty::FloatTy::F32) => scalar.to_f32().discard_err()?.to_string(),
        ty::Float(ty::FloatTy::F64) => scalar.to_f64().discard_err()?.to_string(),
        ty::Float(ty::FloatTy::F128) => scalar.to_f128().discard_err()?.to_string(),
        _ => return format_fallback(ecx, op),
    };
    Some(text)
}

fn format_str<'tcx>(ecx: &MiriInterpCx<'tcx>, op: &OpTy<'tcx>) -> Option<String> {
    let mplace = match ecx.read_immediate_raw(op).discard_err()? {
        Either::Left(mplace) => mplace,
        Either::Right(_) => ecx.deref_pointer(op).discard_err()?,
    };
    let s = ecx.read_str(&mplace).discard_err()?;
    Some(format_str_contents(s))
}

fn format_str_contents(s: &str) -> String {
    if s.len() > MAX_STR { format!("{:?}…", &s[..MAX_STR]) } else { format!("{s:?}") }
}

fn format_ref<'tcx>(
    ecx: &MiriInterpCx<'tcx>,
    op: &OpTy<'tcx>,
    inner: Ty<'tcx>,
    depth: u8,
) -> Option<String> {
    if inner.is_str() {
        let mplace = ecx.deref_pointer(op).discard_err()?;
        let s = ecx.read_str(&mplace).discard_err()?;
        return Some(format_str_contents(s));
    }
    if let ty::Slice(elem) = *inner.kind() {
        let mplace = ecx.deref_pointer(op).discard_err()?;
        let slice = mplace.to_op(ecx).discard_err()?;
        let body = format_array_like(ecx, &slice, depth, ('[', ']'))?;
        if matches!(elem.kind(), ty::Uint(ty::UintTy::U8)) {
            return Some(body);
        }
        return Some(format!("&{body}"));
    }
    let ptr = format_ptr_value(ecx, op)?;
    if is_primitive(inner) {
        if let Some(mplace) = ecx.deref_pointer(op).discard_err() {
            let pointee = mplace.to_op(ecx).discard_err()?;
            if let Some(val) = format_op(ecx, &pointee, depth.saturating_add(1)) {
                return Some(format!("&{val}"));
            }
        }
    }
    Some(format!("&{ptr}"))
}

fn format_raw_ptr<'tcx>(
    ecx: &MiriInterpCx<'tcx>,
    op: &OpTy<'tcx>,
    inner: Ty<'tcx>,
) -> Option<String> {
    let ptr = format_ptr_value(ecx, op)?;
    let prefix = if inner.is_str() { "*const str " } else { "" };
    Some(format!("{prefix}{ptr}"))
}

fn format_fn_ptr<'tcx>(ecx: &MiriInterpCx<'tcx>, op: &OpTy<'tcx>) -> Option<String> {
    format_ptr_value(ecx, op)
}

fn format_ptr_value<'tcx>(ecx: &MiriInterpCx<'tcx>, op: &OpTy<'tcx>) -> Option<String> {
    let imm = ecx.read_immediate(op).discard_err()?;
    match *imm {
        Immediate::Scalar(Scalar::Int(int)) => {
            let bits = int.to_bits(op.layout.size);
            if bits == 0 { Some("null".to_owned()) } else { Some(format!("0x{bits:x}")) }
        }
        Immediate::Scalar(Scalar::Ptr(ptr, _)) => Some(format_strict_ptr(ptr)),
        Immediate::ScalarPair(Scalar::Ptr(ptr, _), meta) => {
            let addr = format_strict_ptr(ptr);
            match meta {
                Scalar::Int(int) => {
                    let len = int.to_bits(ecx.pointer_size());
                    Some(format!("{addr} len={len}"))
                }
                Scalar::Ptr(meta_ptr, _) =>
                    Some(format!("{addr} meta={}", format_strict_ptr(meta_ptr))),
            }
        }
        Immediate::ScalarPair(Scalar::Int(int), _) => {
            let bits = int.to_bits(op.layout.size);
            if bits == 0 { Some("null".to_owned()) } else { Some(format!("0x{bits:x}")) }
        }
        Immediate::Uninit => None,
    }
}

fn format_strict_ptr(ptr: StrictPointer) -> String {
    let (prov, addr) = ptr.into_raw_parts();
    let addr = addr.bytes();
    if addr == 0 {
        return "null".to_owned();
    }
    match prov.get_alloc_id() {
        Some(id) => format!("0x{addr:x} (alloc{})", id.0.get()),
        None => format!("0x{addr:x}"),
    }
}

fn format_enum<'tcx>(
    ecx: &MiriInterpCx<'tcx>,
    op: &OpTy<'tcx>,
    def: ty::AdtDef<'tcx>,
    depth: u8,
) -> Option<String> {
    let enum_name = ecx.tcx.item_name(def.did());
    let variant_idx = ecx.read_discriminant(op).discard_err()?;
    let variant = def.variant(variant_idx);
    let variant_name = variant.name;
    if variant.fields.is_empty() {
        return Some(format!("{enum_name}::{variant_name}"));
    }
    let down = ecx.project_downcast(op, variant_idx).discard_err()?;
    let body = format_adt_fields(ecx, &down, variant, depth)?;
    Some(format!("{enum_name}::{variant_name}{body}"))
}

fn format_struct<'tcx>(
    ecx: &MiriInterpCx<'tcx>,
    op: &OpTy<'tcx>,
    def: ty::AdtDef<'tcx>,
    depth: u8,
) -> Option<String> {
    let variant = def.non_enum_variant();
    let name = ecx.tcx.item_name(def.did());
    if variant.fields.is_empty() {
        return Some(name.to_string());
    }
    let body = format_adt_fields(ecx, op, variant, depth)?;
    Some(format!("{name}{body}"))
}

fn format_adt_fields<'tcx>(
    ecx: &MiriInterpCx<'tcx>,
    op: &OpTy<'tcx>,
    variant: &ty::VariantDef,
    depth: u8,
) -> Option<String> {
    let tuple_like =
        variant.fields.iter().enumerate().all(|(i, field)| field.name.as_str() == i.to_string());
    let mut parts = Vec::new();
    for (i, field) in variant.fields.iter().enumerate() {
        let field_op = ecx.project_field(op, FieldIdx::from_usize(i)).discard_err()?;
        let val = format_op(ecx, &field_op, depth.saturating_add(1))
            .unwrap_or_else(|| "uninit".to_owned());
        if tuple_like {
            parts.push(val);
        } else {
            parts.push(format!("{}: {val}", field.name));
        }
    }
    if tuple_like {
        Some(format!("({})", parts.join(", ")))
    } else {
        Some(format!(" {{ {} }}", parts.join(", ")))
    }
}

fn format_tuple<'tcx>(ecx: &MiriInterpCx<'tcx>, op: &OpTy<'tcx>, depth: u8) -> Option<String> {
    let n = op.layout.fields.count();
    if n == 0 {
        return Some("()".to_owned());
    }
    let mut parts = Vec::new();
    for i in 0..n {
        let field_op = ecx.project_field(op, FieldIdx::from_usize(i)).discard_err()?;
        let val = format_op(ecx, &field_op, depth.saturating_add(1))
            .unwrap_or_else(|| "uninit".to_owned());
        parts.push(val);
    }
    if n == 1 { Some(format!("({},)", parts[0])) } else { Some(format!("({})", parts.join(", "))) }
}

fn format_array_like<'tcx>(
    ecx: &MiriInterpCx<'tcx>,
    op: &OpTy<'tcx>,
    depth: u8,
    (open, close): (char, char),
) -> Option<String> {
    let len = op.len(ecx).discard_err()?;
    let shown = len.min(MAX_ITEMS);
    let mut parts = Vec::new();
    for i in 0..shown {
        let elem = ecx.project_index(op, i).discard_err()?;
        let val =
            format_op(ecx, &elem, depth.saturating_add(1)).unwrap_or_else(|| "uninit".to_owned());
        parts.push(val);
    }
    let mut body = parts.join(", ");
    if len > shown {
        body.push_str(", ...");
    }
    Some(format!("{open}{body}{close}"))
}

fn format_fallback<'tcx>(ecx: &MiriInterpCx<'tcx>, op: &OpTy<'tcx>) -> Option<String> {
    let imm = match ecx.read_immediate_raw(op).discard_err()? {
        Either::Right(imm) => imm,
        Either::Left(_) => return Some(format!("<{}>", op.layout.ty)),
    };
    match *imm {
        Immediate::Uninit => None,
        Immediate::Scalar(Scalar::Ptr(ptr, _)) => Some(format_strict_ptr(ptr)),
        Immediate::Scalar(Scalar::Int(int)) => {
            let bits = int.to_bits(op.layout.size);
            Some(format!("0x{bits:x}"))
        }
        Immediate::ScalarPair(a, b) =>
            Some(format!("({}, {})", format_scalar_short(a), format_scalar_short(b))),
    }
}

fn format_scalar_short(scalar: Scalar) -> String {
    match scalar {
        Scalar::Ptr(ptr, _) => format_strict_ptr(ptr),
        Scalar::Int(int) => format!("0x{:x}", int.to_bits(int.size())),
    }
}

fn is_primitive(ty: Ty<'_>) -> bool {
    matches!(ty.kind(), ty::Bool | ty::Char | ty::Int(_) | ty::Uint(_) | ty::Float(_))
}
