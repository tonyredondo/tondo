//! References an owner gives up, taken instead of counted again: after
//! inlining, a value read out of a local that owns it, as a match binds a
//! payload or a call's result is unwrapped, is retained for where it goes,
//! and the local is released later with nothing having read that part of
//! it since. The retain becomes a move: the part is zeroed in the local,
//! which then releases the rest, and the copy keeps the local's reference.
//! A string decoded from an event went through three atomic operations;
//! now it is moved.
//!
//! The local must own what it holds: neither a parameter, which is the
//! caller's, nor the result, nor what a binding borrows or lends; and on
//! every way on, before the function returns or the part is written, the
//! local is released whole or by a part that holds this one. Nothing on
//! the way may read the part, which is zero then.

use crate::mir::*;

/// How far a way is followed, in statements, before giving up.
const REACH: usize = 4000;

pub fn moves(bodies: &mut [Body], types: &crate::types::Types, sources: &crate::source::SourceMap) {
    crate::parallel::each_mut(bodies, |body| {
        moves_in(body, types);
        // The calls inlined from the standard library, whose bindings a dump
        // of the program's own code need not show.
        let standard: Vec<bool> =
            body.inlined.iter().map(|call| sources.file_of(call.span.start).logical.starts_with("<std>/")).collect();
        drop_empty_values(body, types, &standard);
    });
}

fn moves_in(body: &mut Body, types: &crate::types::Types) {
    if !body.blocks.iter().flat_map(|block| &block.stmts).any(|s| matches!(s, Statement::Retain(_))) {
        return;
    }
    // What is not the function's own to give up: a parameter it borrows,
    // not one it takes (`takes`), and its result.
    let mut foreign = vec![false; body.locals.len()];
    for (at, local) in body.params.iter().enumerate() {
        foreign[local.0 as usize] = !body.takes.get(at).copied().unwrap_or(false);
    }
    if let Some(ret) = body.ret {
        foreign[ret.0 as usize] = true;
    }
    for statement in body.blocks.iter().flat_map(|block| &block.stmts) {
        match statement {
            Statement::Lend(alias, place) => {
                foreign[alias.0 as usize] = true;
                foreign[place.local.0 as usize] = true;
            }
            Statement::LendAt(alias, _) => foreign[alias.0 as usize] = true,
            _ => {}
        }
    }
    for block in 0..body.blocks.len() {
        for at in 0..body.blocks[block].stmts.len() {
            let Statement::Retain(place) = &body.blocks[block].stmts[at] else { continue };
            let Some(source) = copied_from(&body.blocks[block].stmts[..at], place) else { continue };
            if foreign[source.local.0 as usize] || !exact(&source) || !given_up(body, block, at, &source) {
                continue;
            }
            body.blocks[block].stmts[at] = Statement::Moved(source);
        }
    }
    drop_released_moves(body, types);
}

/// A place reached only through fields and variants, which a zero written
/// there leaves the rest of the local's value as it was.
fn exact(place: &Place) -> bool {
    place.path.iter().all(|projection| matches!(projection, Proj::Field(_) | Proj::Variant(..)))
}

/// Where the value retained at `place` was read from, by the statement
/// before that made it, with nothing in between naming either local.
fn copied_from(before: &[Statement], place: &Place) -> Option<Place> {
    let names = |statement: &Statement, local: Local| {
        let (mut used, mut defined) = (Vec::new(), Vec::new());
        statement_locals(statement, &mut used, &mut defined);
        used.contains(&local) || defined.contains(&local)
    };
    for (at, statement) in before.iter().enumerate().rev() {
        if let Statement::Assign(target, value, _) = statement
            && target.path.is_empty()
            && target.local == place.local
        {
            let operand = match (&**value, &place.path[..]) {
                (Rvalue::Use(operand), []) => operand,
                (Rvalue::Variant(variant, operands), [Proj::Variant(of, index)]) if variant == of => {
                    operands.get(*index as usize)?
                }
                (Rvalue::Aggregate(operands), [Proj::Field(index)]) => operands.get(*index as usize)?,
                _ => return None,
            };
            let Operand::Copy(source) = operand else { return None };
            let untouched = !before[at + 1..].iter().any(|statement| names(statement, source.local));
            return (source.local != place.local && untouched).then(|| source.clone());
        }
        if names(statement, place.local) {
            return None;
        }
    }
    None
}

/// Whether `part` covers `place`: the same place, or one inside it.
fn covers(part: &Place, place: &Place) -> bool {
    part.local == place.local && place.path.starts_with(&part.path)
}

/// Whether two places may share memory: one inside the other, or in two
/// variants of one enum, whose payloads overlap.
fn overlap(a: &Place, b: &Place) -> bool {
    if a.local != b.local {
        return false;
    }
    for (x, y) in a.path.iter().zip(&b.path) {
        match (x, y) {
            (Proj::Field(i), Proj::Field(j)) if i != j => return false,
            (Proj::Variant(v, i), Proj::Variant(w, j)) if v == w && i != j => return false,
            _ => {}
        }
    }
    true
}

/// Whether, on every way on from the retain at `at` in `block`, `source`
/// is released, by itself or a place that holds it, before anything reads
/// it, writes it or the function returns. A local copied whole into
/// another that then takes its place, as `x = Variant(x, y)` keeps the old
/// value of `x` to release it once the new one is in, hands the value on:
/// what is followed then is that other local.
fn given_up(body: &Body, block: usize, at: usize, source: &Place) -> bool {
    // Each block once for each local followed into it.
    let mut seen: std::collections::HashSet<(usize, Local)> = std::collections::HashSet::new();
    let mut ways = vec![(block, at + 1, source.clone())];
    let mut steps = 0;
    while let Some((block, start, mut source)) = ways.pop() {
        let data = &body.blocks[block];
        let mut released = false;
        for (index, statement) in data.stmts.iter().enumerate().skip(start) {
            steps += 1;
            if steps > REACH {
                return false;
            }
            if let Some(holder) = handed_on(&data.stmts[index..], &source) {
                source = holder;
                continue;
            }
            match step(statement, &source) {
                Step::Released => {
                    released = true;
                    break;
                }
                Step::Conflict => return false,
                Step::Unrelated => {}
            }
        }
        if released {
            continue;
        }
        let mut read = false;
        crate::inline::visit_terminator(&mut data.terminator.clone(), &mut |place: &mut Place| {
            read |= overlap(place, &source);
        });
        if read {
            return false;
        }
        let way = |to: usize, seen: &mut std::collections::HashSet<(usize, Local)>| {
            seen.insert((to, source.local)).then(|| (to, 0, source.clone()))
        };
        match &data.terminator {
            Terminator::Goto(to) => ways.extend(way(to.0 as usize, &mut seen)),
            Terminator::Branch(_, then, otherwise) => {
                ways.extend(way(then.0 as usize, &mut seen));
                ways.extend(way(otherwise.0 as usize, &mut seen));
            }
            // Its reference would leave with the function, or the local
            // never owned one.
            Terminator::Return(_) => return false,
            Terminator::Panic { .. } | Terminator::Unreachable => {}
        }
    }
    true
}

/// Where `source` is held once the first of `statements` copies its local
/// whole into another and the next statement naming its local writes it
/// whole without reading it, as `old = x; x = new; release old` does: its
/// old value, and the part `source` names, are the other local's from then
/// on.
fn handed_on(statements: &[Statement], source: &Place) -> Option<Place> {
    let Statement::Assign(target, value, _) = &statements[0] else { return None };
    let Rvalue::Use(Operand::Copy(from)) = &**value else { return None };
    if !from.path.is_empty() || !target.path.is_empty() || from.local != source.local || target.local == source.local {
        return None;
    }
    let names = |statement: &Statement| {
        let (mut used, mut defined) = (Vec::new(), Vec::new());
        statement_locals(statement, &mut used, &mut defined);
        (used.contains(&source.local), defined.contains(&source.local))
    };
    let next = statements[1..].iter().find(|statement| names(statement) != (false, false))?;
    match names(next) {
        (false, true) => Some(Place { local: target.local, path: source.path.clone() }),
        _ => None,
    }
}

enum Step {
    Released,
    Conflict,
    Unrelated,
}

/// What a statement does with `source`, zeroed after the move.
fn step(statement: &Statement, source: &Place) -> Step {
    match statement {
        Statement::Release(place) if covers(place, source) => return Step::Released,
        // Zeroing it again changes nothing.
        Statement::Moved(place) if overlap(place, source) => return Step::Unrelated,
        // A tag is not the payload moved out of it.
        Statement::Assign(target, value, _) if matches!(&**value, Rvalue::Discriminant(of) if !covers(source, of)) => {
            return if overlap(target, source) { Step::Conflict } else { Step::Unrelated };
        }
        _ => {}
    }
    let mut conflict = false;
    let mut statement = statement.clone();
    crate::inline::visit_statement(&mut statement, &mut |place: &mut Place| {
        if overlap(place, source) {
            conflict = true;
        }
    });
    let cleared = matches!(statement, Statement::Clear(local) if local == source.local);
    if conflict || cleared { Step::Conflict } else { Step::Unrelated }
}

/// A temporary only ever made of variants that hold nothing to release, and
/// then released, with nothing reading it: as inlining leaves the result of
/// `math.checkedSqrt(x)`, `Float ! MathError`, whose error may hold a text, after
/// `??` took its value from where it was made. Its releases, which
/// read its tag to find nothing to free, and what it is made of, go.
fn drop_empty_values(body: &mut Body, types: &crate::types::Types, standard: &[bool]) {
    let count = body.locals.len();
    // Where each local is named, and whether a terminator names it.
    let mut named: Vec<Vec<(usize, usize)>> = vec![Vec::new(); count];
    let mut in_terminator = vec![false; count];
    for (block, data) in body.blocks.iter().enumerate() {
        for (at, statement) in data.stmts.iter().enumerate() {
            let (mut used, mut defined) = (Vec::new(), Vec::new());
            statement_locals(statement, &mut used, &mut defined);
            used.extend(defined);
            used.sort_by_key(|local| local.0);
            used.dedup();
            for local in used {
                named[local.0 as usize].push((block, at));
            }
        }
        let mut used = Vec::new();
        terminator_uses(&data.terminator, &mut used);
        for local in used {
            in_terminator[local.0 as usize] = true;
        }
    }
    // The program's own bindings stay, as a dump shows them, those of its
    // code inlined here too, and so do what the function is given and
    // gives. One of the standard library's code inlined here that nothing
    // writes then, a dump leaves out, as an argument the inliner found
    // unused.
    let mut kept = vec![false; count];
    for local in body.params.iter().chain(&body.ret).chain(&body.framed) {
        kept[local.0 as usize] = true;
    }
    for (local, _, scope) in &body.scoped {
        if !standard.get(*scope as usize).copied().unwrap_or(false) {
            kept[local.0 as usize] = true;
        }
    }
    for (index, name) in body.names.iter().enumerate() {
        kept[index] |= name.is_some();
    }
    let free = |local: usize| !kept[local] && !in_terminator[local];
    fn statement(body: &Body, (block, at): (usize, usize)) -> &Statement {
        &body.blocks[block].stmts[at]
    }
    // A temporary named, besides at `held`, only where it is made of a
    // variant without anything: the one thing another variant holds.
    let bare = |body: &Body, local: usize, held: (usize, usize)| {
        free(local)
            && named[local].iter().filter(|&&at| at == held).count() == 1
            && named[local].iter().filter(|&&at| at != held).count() >= 1
            && named[local].iter().filter(|&&at| at != held).all(|&at| match statement(body, at) {
                Statement::Assign(place, rvalue, _) => {
                    *place == Place::local(Local(local as u32))
                        && matches!(&**rvalue, Rvalue::Variant(_, operands) if operands.is_empty())
                }
                _ => false,
            })
    };
    let mut gone: Vec<(usize, usize)> = Vec::new();
    for local in 0..count {
        let ty = body.locals[local];
        if !free(local) || !types.needs_drop(ty) || named[local].is_empty() {
            continue;
        }
        let mut parts = Vec::new();
        let empty = named[local].iter().all(|&at| match statement(body, at) {
            Statement::Release(place) => *place == Place::local(Local(local as u32)),
            Statement::Assign(place, rvalue, _) if *place == Place::local(Local(local as u32)) => {
                let Rvalue::Variant(_, operands) = &**rvalue else { return false };
                operands.iter().all(|operand| match operand {
                    Operand::Const(_) => true,
                    Operand::Copy(place) if !place.path.is_empty() => false,
                    Operand::Copy(place) if !types.needs_drop(body.locals[place.local.0 as usize]) => true,
                    Operand::Copy(place) => {
                        let part = place.local.0 as usize;
                        let ok = part != local && bare(body, part, at);
                        if ok {
                            parts.push(part);
                        }
                        ok
                    }
                })
            }
            _ => false,
        });
        if !empty {
            continue;
        }
        gone.extend(named[local].iter().copied());
        for part in parts {
            gone.extend(named[part].iter().copied().filter(
                |&at| matches!(statement(body, at), Statement::Assign(place, ..) if place.local.0 as usize == part),
            ));
        }
    }
    if gone.is_empty() {
        return;
    }
    gone.sort_unstable();
    gone.dedup();
    for &(block, at) in gone.iter().rev() {
        body.blocks[block].stmts.remove(at);
    }
    crate::mir_text::note(body, "moves", || {
        format!("dropped {} statements of values with nothing to release", gone.len())
    });
}

/// Whether `place` is the only field of an enum variant (including `Option`
/// and `Result`) or of a record/tuple that needs dropping, so moving it leaves
/// releasing the whole local as a no-op.
fn sole_counted_part(types: &crate::types::Types, local_ty: crate::types::Ty, path: &[Proj]) -> bool {
    match path {
        [Proj::Variant(variant, field_index)] => {
            let Some(variants) = types.variants(local_ty) else { return false };
            let Some(fields) = variants.get(*variant as usize) else { return false };
            (*field_index as usize) < fields.len()
                && fields.iter().enumerate().all(|(i, &ty)| i == *field_index as usize || !types.needs_drop(ty))
        }
        [Proj::Field(field_index)] => {
            let Some(fields) = types.components(local_ty) else { return false };
            (*field_index as usize) < fields.len()
                && fields.iter().enumerate().all(|(i, &ty)| i == *field_index as usize || !types.needs_drop(ty))
        }
        _ => false,
    }
}

/// Whether `local` is never read, modified in part, retained, released or lent
/// on any forward path from `(block, from)` before being overwritten whole or
/// cleared.
fn dead_after(body: &Body, block: usize, from: usize, local: Local) -> bool {
    let mut seen = vec![false; body.blocks.len()];
    let mut work = vec![(block, from)];
    while let Some((b, start)) = work.pop() {
        let data = &body.blocks[b];
        let mut overwritten = false;
        for statement in data.stmts.iter().skip(start) {
            let (mut used, mut defined) = (Vec::new(), Vec::new());
            statement_locals(statement, &mut used, &mut defined);
            if matches!(statement, Statement::Clear(cleared) if *cleared == local) {
                overwritten = true;
                break;
            }
            if let Statement::Assign(target, _, _) = statement
                && target.local == local
                && target.path.is_empty()
                && !used.contains(&local)
            {
                overwritten = true;
                break;
            }
            if used.contains(&local) || defined.contains(&local) {
                return false;
            }
        }
        if overwritten {
            continue;
        }
        let mut used = Vec::new();
        terminator_uses(&data.terminator, &mut used);
        if used.contains(&local) {
            return false;
        }
        for next in successors(&data.terminator) {
            if !seen[next] {
                seen[next] = true;
                work.push((next, 0));
            }
        }
    }
    true
}

/// Drops releases of locals whose whole value or sole counted part has already
/// been moved out (or cleared) on every reachable path to the release, and
/// drops zeroing moves whose local is dead on every path forward.
fn drop_released_moves(body: &mut Body, types: &crate::types::Types) {
    let names = |statement: &Statement, local: Local| {
        let (mut used, mut defined) = (Vec::new(), Vec::new());
        statement_locals(statement, &mut used, &mut defined);
        used.contains(&local) || defined.contains(&local)
    };
    // First keep the exact whole-local same-block removal (so dead single-use
    // whole-local moves + releases disappear together cleanly).
    for block in 0..body.blocks.len() {
        let mut at = 0;
        while at < body.blocks[block].stmts.len() {
            let statements = &body.blocks[block].stmts;
            let Statement::Moved(place) = &statements[at] else {
                at += 1;
                continue;
            };
            let local = place.local;
            let release = statements[at + 1..]
                .iter()
                .position(|statement| names(statement, local))
                .map(|index| at + 1 + index)
                .filter(|&index| matches!(&statements[index], Statement::Release(r) if *r == Place::local(local)));
            let elsewhere = |body: &Body, release: usize| {
                body.blocks.iter().enumerate().any(|(index, data)| {
                    let statements = if index == block { &data.stmts[release + 1..] } else { &data.stmts[..] };
                    let mut used = Vec::new();
                    terminator_uses(&data.terminator, &mut used);
                    used.contains(&local) || statements.iter().any(|statement| names(statement, local))
                })
            };
            match release {
                Some(release) if place.path.is_empty() && !elsewhere(body, release) => {
                    body.blocks[block].stmts.remove(release);
                    body.blocks[block].stmts.remove(at);
                }
                _ => at += 1,
            }
        }
    }

    // Forward must-analysis across reachable blocks: a bit is set when every
    // reachable path to this point has moved out the local's whole value or
    // sole counted part (or cleared it) without writing it since.
    let count = body.locals.len();
    if count == 0 || body.blocks.is_empty() {
        return;
    }
    let mut tracked = vec![true; count];
    for &param in &body.params {
        tracked[param.0 as usize] = false;
    }
    if let Some(ret) = body.ret {
        tracked[ret.0 as usize] = false;
    }
    for (i, &ty) in body.locals.iter().enumerate() {
        if !types.needs_drop(ty) {
            tracked[i] = false;
        }
    }
    let mut any_moved = false;
    for data in &body.blocks {
        for statement in &data.stmts {
            match statement {
                Statement::Lend(alias, place) => {
                    tracked[alias.0 as usize] = false;
                    tracked[place.local.0 as usize] = false;
                }
                Statement::LendAt(alias, _) => tracked[alias.0 as usize] = false,
                Statement::DeferPush(_, place) => tracked[place.local.0 as usize] = false,
                Statement::Moved(place) if tracked[place.local.0 as usize] => any_moved = true,
                _ => {}
            }
        }
    }
    if !any_moved {
        return;
    }

    let n_blocks = body.blocks.len();
    let mut reachable = vec![false; n_blocks];
    let mut preds: Vec<Vec<usize>> = vec![Vec::new(); n_blocks];
    let mut stack = vec![0usize];
    reachable[0] = true;
    while let Some(b) = stack.pop() {
        for next in successors(&body.blocks[b].terminator) {
            preds[next].push(b);
            if !reachable[next] {
                reachable[next] = true;
                stack.push(next);
            }
        }
    }

    let words = count.div_ceil(64);
    let set_bit = |bits: &mut [u64], local: Local| {
        let idx = local.0 as usize;
        bits[idx / 64] |= 1u64 << (idx % 64);
    };
    let clear_bit = |bits: &mut [u64], local: Local| {
        let idx = local.0 as usize;
        bits[idx / 64] &= !(1u64 << (idx % 64));
    };
    let has_bit = |bits: &[u64], local: Local| -> bool {
        let idx = local.0 as usize;
        (bits[idx / 64] & (1u64 << (idx % 64))) != 0
    };
    let transfer = |state: &mut Vec<u64>, statement: &Statement| match statement {
        Statement::Moved(place)
            if tracked[place.local.0 as usize]
                && (place.path.is_empty()
                    || sole_counted_part(types, body.locals[place.local.0 as usize], &place.path)) =>
        {
            set_bit(state, place.local);
        }
        Statement::Clear(local) if tracked[local.0 as usize] => set_bit(state, *local),
        Statement::Assign(place, rvalue, _) => {
            clear_bit(state, place.local);
            match &**rvalue {
                Rvalue::Mutate(_, target, ..) | Rvalue::AssignSlice(target, ..) => clear_bit(state, target.local),
                Rvalue::Select(cases, ..) => {
                    for case in cases {
                        clear_bit(state, case.out);
                    }
                }
                _ => {}
            }
        }
        Statement::Retain(place) => clear_bit(state, place.local),
        Statement::OsrLoad(_) => state.fill(0),
        _ => {}
    };

    let mut entry: Vec<Vec<u64>> = vec![vec![0; words]; n_blocks];
    let mut exit: Vec<Option<Vec<u64>>> = vec![None; n_blocks];
    let mut changed = true;
    while changed {
        changed = false;
        for b in 0..n_blocks {
            if !reachable[b] {
                continue;
            }
            let mut cur = if b == 0 {
                vec![0u64; words]
            } else {
                let mut acc: Option<Vec<u64>> = None;
                for &p in &preds[b] {
                    if let Some(p_exit) = &exit[p] {
                        match &mut acc {
                            None => acc = Some(p_exit.clone()),
                            Some(bits) => {
                                for (a, &e) in bits.iter_mut().zip(p_exit) {
                                    *a &= e;
                                }
                            }
                        }
                    }
                }
                let Some(acc) = acc else { continue };
                acc
            };
            entry[b].clone_from(&cur);
            for statement in &body.blocks[b].stmts {
                transfer(&mut cur, statement);
            }
            if exit[b].as_ref() != Some(&cur) {
                exit[b] = Some(cur);
                changed = true;
            }
        }
    }

    let mut dropped_releases = 0usize;
    for (b, &is_reachable) in reachable.iter().enumerate() {
        if !is_reachable {
            continue;
        }
        let mut cur = entry[b].clone();
        let mut keep = Vec::with_capacity(body.blocks[b].stmts.len());
        for statement in &body.blocks[b].stmts {
            let dead = matches!(statement, Statement::Release(r) if r.path.is_empty() && has_bit(&cur, r.local));
            keep.push(!dead);
            if dead {
                dropped_releases += 1;
            } else {
                transfer(&mut cur, statement);
            }
        }
        if keep.iter().any(|&k| !k) {
            let mut it = keep.into_iter();
            body.blocks[b].stmts.retain(|_| it.next().unwrap());
        }
    }

    if dropped_releases == 0 {
        return;
    }

    // Any partial `Moved` whose local is now dead on every forward path no
    // longer needs to zero the payload slot either.
    for (b, &is_reachable) in reachable.iter().enumerate() {
        if !is_reachable {
            continue;
        }
        let mut at = 0;
        while at < body.blocks[b].stmts.len() {
            let drop_move = matches!(
                &body.blocks[b].stmts[at],
                Statement::Moved(place)
                    if !place.path.is_empty()
                        && tracked[place.local.0 as usize]
                        && sole_counted_part(types, body.locals[place.local.0 as usize], &place.path)
                        && dead_after(body, b, at + 1, place.local)
            );
            if drop_move {
                body.blocks[b].stmts.remove(at);
            } else {
                at += 1;
            }
        }
    }
}
