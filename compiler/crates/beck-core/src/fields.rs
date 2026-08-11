//! Where a record literal's fields go, decided once instead of once per record.
//!
//! A record's fields are held in one order — by name — because that order is the `Map`'s
//! iteration, the state digest and the patch stream a replay has to reproduce bit for bit
//! ([`54`](../../../../../docs/54-ordering.md)). A record *literal* is written in some other
//! order, usually the declaration's, so building one meant sorting three to eight names on every
//! construction.
//!
//! The names are in the source, so the answer is the same every time the literal runs. This pass
//! computes it once and writes it on the node: [`crate::core::Core::order`] holds, for each
//! position of the finished record, which written field belongs there — four bits each, so a
//! literal of up to [`MAX_ORDERED`] fields carries its whole layout in a `u32` that fits in
//! padding `Core` already had.
//!
//! Evaluation order is **not** what changes. A field expression can `raise`, so the values are
//! still computed in the order they are written; what the permutation removes is the comparing
//! and moving afterwards.
//!
//! Like [`crate::frames`], being unable to answer is safe: [`UNORDERED`] means "sort at run
//! time", and it is what a literal with a repeated field name, one with more fields than fit, and
//! every program built by something that never runs this pass all carry.

use crate::core::{Core, CoreKind};

/// No layout on this node: whoever builds the record sorts it, as it did before this pass existed.
///
/// It cannot collide with a real layout. Every nibble of one is a written field's index, which is
/// below [`MAX_ORDERED`], so no layout has `0xf` anywhere and this one is `0xf` everywhere.
pub const UNORDERED: u32 = u32::MAX;

/// How many fields fit in the packed layout: eight, at four bits each.
///
/// Nothing in this tree declares a record wider than that, and one that did would sort at run time
/// rather than fail — which is why this is a constant here and not a limit in the language.
pub const MAX_ORDERED: usize = 8;

/// Put `items` — a record literal's fields, in the order they are written — into the order the
/// record holds them in.
///
/// **One allocation, and it is the caller's.** That is the whole design constraint: the vector of
/// evaluated fields already exists, so this permutes it where it lies rather than selecting out of
/// it into a second one. Building the answer into a fresh vector was tried and measured, and it
/// cost more than the sort it replaced — an allocation is dearer than an insertion sort over four
/// names ([`70`](../../../../../docs/70-the-evaluator-gets-fast-report.md) §70.4).
///
/// The permutation is followed in its cycles, which needs somewhere to record what has already
/// been placed; here that is the packed layout itself, copied into a local and rewritten as it
/// goes. So the bookkeeping is a `u32` in a register and the whole of this touches no memory but
/// the elements it moves.
///
/// The caller has established that the node carries a layout ([`UNORDERED`] does not) and that the
/// record is no wider than [`MAX_ORDERED`].
pub fn place<T>(items: &mut [T], order: u32) {
    let mut dest = order;
    for i in 0..items.len() {
        loop {
            let j = nibble(dest, i);
            if j == i {
                break;
            }
            items.swap(i, j);
            dest = swap_nibbles(dest, i, j);
        }
    }
}

/// Where the field written at position `at` belongs in the finished record.
#[inline]
fn nibble(order: u32, at: usize) -> usize {
    ((order >> (4 * at)) & 0xf) as usize
}

#[inline]
fn swap_nibbles(order: u32, i: usize, j: usize) -> u32 {
    let (a, b) = (nibble(order, i) as u32, nibble(order, j) as u32);
    let cleared = order & !(0xf << (4 * i)) & !(0xf << (4 * j));
    cleared | (b << (4 * i)) | (a << (4 * j))
}

/// Give every record literal in the program its layout.
pub fn order_program(program: &mut crate::check::Program) {
    for def in program.defs.values_mut() {
        order(&mut def.body);
    }
    for test in program.tests.iter_mut() {
        for c in test.cores_mut() {
            order(c);
        }
    }
}

/// The same for one expression, and everything under it.
pub fn order(c: &mut Core) {
    order_here(c);
    for child in crate::core::children_mut(c) {
        order(child);
    }
}

/// The layout of this node alone, for a record synthesised after the pass has run — the splitter's
/// fused state is the one that is.
pub fn order_here(c: &mut Core) {
    if let CoreKind::Make { fields, .. } = &c.kind {
        c.order = layout(fields.iter().map(|(name, _)| name.as_ref()));
    }
}

/// The packed layout for these field names, or [`UNORDERED`] when there is not one to give.
fn layout<'a>(names: impl Iterator<Item = &'a str>) -> u32 {
    let names: Vec<&str> = names.collect();
    if names.is_empty() || names.len() > MAX_ORDERED {
        return UNORDERED;
    }
    let mut by_name: Vec<usize> = (0..names.len()).collect();
    by_name.sort_unstable_by(|&i, &j| crate::core::cmp_name(names[i], names[j]));
    // A repeated name has no layout: two written fields would want one position, and which of them
    // wins is a question the run-time sort answers today and this pass must not answer differently.
    if by_name
        .windows(2)
        .any(|w| crate::core::cmp_name(names[w[0]], names[w[1]]) != std::cmp::Ordering::Less)
    {
        return UNORDERED;
    }
    let mut order = 0u32;
    for (at, &written) in by_name.iter().enumerate() {
        order |= (at as u32) << (4 * written);
    }
    order
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::{Const, VarId};
    use crate::ty::Ty;
    use beck_diag::Span;
    use std::sync::Arc;

    fn make(names: &[&str]) -> Core {
        Core::new(
            CoreKind::Make {
                ty: "R".into(),
                variant: None,
                fields: names
                    .iter()
                    .map(|n| {
                        (
                            Arc::from(*n),
                            Core::new(CoreKind::Const(Const::Int(0)), Ty::int(), Span::NONE),
                        )
                    })
                    .collect(),
            },
            Ty::int(),
            Span::NONE,
        )
    }

    /// Apply a node's layout to its own field names, which is what the evaluator does to their
    /// values.
    fn placed(c: &Core, names: &[&str]) -> Vec<String> {
        let mut items: Vec<String> = names.iter().map(|s| s.to_string()).collect();
        place(&mut items, c.order);
        items
    }

    #[test]
    fn a_literal_written_in_order_is_the_identity() {
        let mut c = make(&["a", "b", "c"]);
        order(&mut c);
        assert_eq!(placed(&c, &["a", "b", "c"]), ["a", "b", "c"]);
    }

    /// `Ball(x=…, y=…, x_vel=…, y_vel=…)` — declaration order, which is not name order.
    #[test]
    fn a_literal_written_in_declaration_order_permutes() {
        let mut c = make(&["x", "y", "x_vel", "y_vel"]);
        order(&mut c);
        assert_eq!(
            placed(&c, &["x", "y", "x_vel", "y_vel"]),
            ["x", "x_vel", "y", "y_vel"]
        );
    }

    /// Every permutation of eight names, placed — because a cycle-following permutation is exactly
    /// the kind of code that is right for the shapes somebody thought to write down and wrong for
    /// one of the 40,320 they did not.
    #[test]
    fn every_permutation_of_a_full_record_places_correctly() {
        fn perms(items: &mut Vec<usize>, k: usize, out: &mut Vec<Vec<usize>>) {
            if k == items.len() {
                out.push(items.clone());
                return;
            }
            for i in k..items.len() {
                items.swap(k, i);
                perms(items, k + 1, out);
                items.swap(k, i);
            }
        }
        let names = ["a", "b", "c", "d", "e", "f", "g", "h"];
        let mut all = Vec::new();
        perms(&mut (0..names.len()).collect(), 0, &mut all);
        assert_eq!(all.len(), 40_320);
        for p in all {
            let written: Vec<&str> = p.iter().map(|&i| names[i]).collect();
            let mut c = make(&written);
            order(&mut c);
            assert_eq!(placed(&c, &written), names, "written as {written:?}");
        }
    }

    #[test]
    fn a_repeated_name_has_no_layout() {
        let mut c = make(&["a", "b", "a"]);
        order(&mut c);
        assert_eq!(c.order, UNORDERED);
    }

    #[test]
    fn a_record_wider_than_the_packing_has_no_layout() {
        let wide: Vec<String> = (0..MAX_ORDERED + 1).map(|i| format!("f{i}")).collect();
        let refs: Vec<&str> = wide.iter().map(|s| s.as_str()).collect();
        let mut c = make(&refs);
        order(&mut c);
        assert_eq!(c.order, UNORDERED);
    }

    /// The point of the sentinel: it is not reachable as an answer, so "no layout" and "this
    /// layout" cannot be confused.
    #[test]
    fn a_layout_is_never_the_sentinel() {
        for n in 1..=MAX_ORDERED {
            let names: Vec<String> = (0..n).map(|i| format!("f{}", MAX_ORDERED - i)).collect();
            let refs: Vec<&str> = names.iter().map(|s| s.as_str()).collect();
            let mut c = make(&refs);
            order(&mut c);
            assert_ne!(c.order, UNORDERED);
        }
    }

    /// A literal nested inside a lambda gets its layout too — the walk is what this checks, since
    /// a lambda's body is behind an `Arc` and is the one child that needs unsharing.
    #[test]
    fn a_literal_inside_a_lambda_is_reached() {
        let inner = make(&["b", "a"]);
        let mut lam = Core::new(
            CoreKind::Lam {
                params: Arc::from(vec![0 as VarId]),
                body: Arc::new(inner),
            },
            Ty::int(),
            Span::NONE,
        );
        order(&mut lam);
        let CoreKind::Lam { body, .. } = &lam.kind else {
            unreachable!()
        };
        assert_eq!(placed(body, &["b", "a"]), ["a", "b"]);
    }
}
