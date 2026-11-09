//! Tasks, joins, groups, channels and `select`.

use super::*;

impl<'a, 'b> FnCodegen<'a, 'b> {
    /// The `T?` result of a removal: `some` with the element the runtime
    /// moved to `out` when `found` is 1, otherwise `none`.
    /// An operation of a `Join` or a `Group` (see the runtime's `tasks`):
    /// the value to store at `place`, or `None` once it is stored.
    pub(super) fn task_op(
        &mut self,
        op: TaskOp,
        operands: &[Operand],
        retain: &[bool],
        place: &Place,
        span: Span,
    ) -> Option<Val> {
        let ty = self.place_ty(place);
        match op {
            TaskOp::Shared(shared) => return self.shared_op(shared, operands, retain, place),
            TaskOp::Atomic(atomic) => return Some(self.atomic_op(atomic, operands, ty, span)),
            TaskOp::Zeroed => return Some(self.zeroed(ty)),
            TaskOp::SyncNew => {
                let TyKind::Handle(handle, state) = *self.e.cg.types().kind(ty) else { unreachable!() };
                if handle.collection() {
                    return Some(Val::Scalar(self.shared_new(handle, state, &operands[0], retain[0])));
                }
            }
            _ => {}
        }
        let handle_ty = operands.first().map(|o| self.operand_ty(o));
        let item = handle_ty.map(|t| self.e.cg.types().handle_result(t));
        // Handles and indices are scalars; the value `send` moves is read
        // where it is used.
        let scalars = match op {
            TaskOp::Send | TaskOp::Offer | TaskOp::TryOffer => &operands[..1],
            TaskOp::SyncNew | TaskOp::RefNew => &operands[..0],
            _ => operands,
        };
        let values: Vec<ir::Value> = scalars.iter().map(|o| self.scalar(o)).collect();
        match op {
            TaskOp::Wait => {
                let item = item.expect("a join");
                let out = self.scratch(item);
                self.e.call_rt(Rt::JoinWait, &[values[0], out]);
                Some(self.e.value_at(out, item))
            }
            TaskOp::GroupNew => {
                let result = self.e.cg.types().handle_result(ty);
                let fallible = matches!(self.e.cg.types().kind(result), TyKind::Result(..));
                let fallible = self.e.fb.ins().iconst(types::I8, fallible as i64);
                Some(Val::Scalar(self.e.call_rt(Rt::GroupNew, &[fallible]).unwrap()))
            }
            TaskOp::GroupAdd => {
                if retain[1] {
                    self.e.call_rt(Rt::HandleRetain, &[values[1]]);
                }
                self.e.call_rt(Rt::GroupAdd, &[values[0], values[1]]);
                None
            }
            TaskOp::GroupNext | TaskOp::GroupTake => {
                let item = item.expect("a group");
                let out = self.scratch(item);
                let found = if op == TaskOp::GroupNext {
                    self.e.call_rt(Rt::GroupNext, &[values[0], out]).unwrap()
                } else {
                    self.e.call_rt(Rt::GroupTake, &[values[0], values[1], out]).unwrap()
                };
                self.removal_result(place, ty, item, out, found)
            }
            TaskOp::GroupLength => Some(Val::Scalar(self.e.call_rt(Rt::GroupLength, &[values[0]]).unwrap())),
            TaskOp::GroupSettle => Some(Val::Scalar(self.e.call_rt(Rt::GroupSettle, &[values[0]]).unwrap())),
            TaskOp::Channel => {
                let sender_ty = self.e.cg.types().components(ty).expect("a pair of ends")[0];
                let value = self.e.cg.types().handle_result(sender_ty);
                let discard = values[0];
                let (capacity, bounded) = match values.get(1) {
                    Some(&capacity) => (capacity, 1),
                    None => (self.e.fb.ins().iconst(types::I64, 0), 0),
                };
                let bounded = self.e.fb.ins().iconst(types::I8, bounded);
                let layout = self.e.cg.layout(value);
                let size = self.e.fb.ins().iconst(POINTER, layout.size as i64);
                let align = self.e.fb.ins().iconst(POINTER, layout.align as i64);
                let release = self.e.glue_or_null(Glue::Release, value);
                let visit = self.e.glue_or_null(Glue::Visit, value);
                let location = self.location(span);
                let channel = self
                    .e
                    .call_rt(Rt::ChannelNew, &[capacity, bounded, size, align, release, visit, location, discard])
                    .unwrap();
                self.live_object(channel, value, 2);
                let sender = self.e.call_rt(Rt::SenderNew, &[channel]).unwrap();
                let pair = self.scratch(ty);
                for (index, end) in [sender, channel].into_iter().enumerate() {
                    let slot = self.e.cg.slot(ty, Proj::Field(index as u32));
                    self.e.fb.ins().store(flags(), end, pair, slot.offset as i32);
                }
                Some(Val::Memory(pair))
            }
            TaskOp::Send => {
                let item = item.expect("a sender");
                let value = self.operand(&operands[1]);
                if retain[1] {
                    self.e.refcount(item, value, false);
                }
                let source = self.copy_to_stack(item, value);
                let location = self.location(span);
                self.e.call_rt(Rt::ChannelSend, &[values[0], source, location]);
                None
            }
            TaskOp::Offer | TaskOp::TryOffer => {
                let item = item.expect("a sender");
                let value = self.operand(&operands[1]);
                if retain[1] {
                    self.e.refcount(item, value, false);
                }
                let source = self.copy_to_stack(item, value);
                let wait = self.e.fb.ins().iconst(types::I8, (op == TaskOp::Offer) as i64);
                Some(Val::Scalar(self.e.call_rt(Rt::ChannelOffer, &[values[0], source, wait]).unwrap()))
            }
            TaskOp::Receive | TaskOp::ReceiveTimeout | TaskOp::WaitTimeout => {
                let item = item.expect("a receiver or a join");
                let out = self.scratch(item);
                let found = match op {
                    TaskOp::Receive => self.e.call_rt(Rt::ChannelReceive, &[values[0], out]),
                    TaskOp::ReceiveTimeout => self.e.call_rt(Rt::ReceiveTimeout, &[values[0], out, values[1]]),
                    _ => self.e.call_rt(Rt::JoinWaitTimeout, &[values[0], out, values[1]]),
                };
                self.removal_result(place, ty, item, out, found.unwrap())
            }
            TaskOp::Cancel => {
                self.e.call_rt(Rt::JoinCancel, &[values[0]]);
                None
            }
            TaskOp::Detach => {
                self.e.call_rt(Rt::JoinDetach, &[values[0]]);
                None
            }
            TaskOp::GroupCancel => {
                self.e.call_rt(Rt::GroupCancel, &[values[0]]);
                None
            }
            TaskOp::SyncNew => {
                // The state moves into the new cell.
                let state = self.e.cg.types().handle_result(ty);
                let value = self.operand(&operands[0]);
                if retain[0] {
                    self.e.refcount(state, value, false);
                }
                let layout = self.e.cg.layout(state);
                let size = self.e.fb.ins().iconst(POINTER, layout.size as i64);
                let align = self.e.fb.ins().iconst(POINTER, layout.align as i64);
                let release = self.e.glue_or_null(Glue::Release, state);
                let visit = self.e.glue_or_null(Glue::Visit, state);
                let cell = self.e.call_rt(Rt::SyncNew, &[size, align, release, visit]).unwrap();
                let address = self.e.call_rt(Rt::SyncValue, &[cell]).unwrap();
                self.e.store_val(address, state, value);
                self.live_object(cell, state, 1);
                Some(Val::Scalar(cell))
            }
            TaskOp::WeakNew => Some(Val::Scalar(self.e.call_rt(Rt::WeakNew, &[values[0]]).unwrap())),
            TaskOp::WeakGet => {
                let reference = self.e.call_rt(Rt::WeakGet, &[values[0]]).unwrap();
                let reference_ty = self.e.cg.types().lookup(&TyKind::Handle(Handle::Ref, item.unwrap())).unwrap();
                self.pointer_option(place, ty, reference_ty, reference)
            }
            TaskOp::RefNew => {
                // The value moves into the new identity.
                let inner = self.e.cg.types().handle_result(ty);
                let value = self.operand(&operands[0]);
                if retain[0] {
                    self.e.refcount(inner, value, false);
                }
                let size = self.e.fb.ins().iconst(POINTER, self.e.cg.layout(inner).size as i64);
                let object = self.e.call_rt(Rt::RefNew, &[size]).unwrap();
                let address = self.e.ref_value(object);
                self.e.store_val(address, inner, value);
                self.live_object(object, inner, 0);
                Some(Val::Scalar(object))
            }
            TaskOp::Lock(mode) => {
                let mode = match mode {
                    LockMode::Shared => 0,
                    LockMode::Exclusive => 1,
                    LockMode::Try => 2,
                };
                let mode = self.e.fb.ins().iconst(types::I8, mode);
                let location = self.location(span);
                Some(Val::Scalar(self.e.call_rt(Rt::SyncLock, &[values[0], mode, location]).unwrap()))
            }
            TaskOp::SyncAwait => Some(Val::Scalar(self.e.call_rt(Rt::SyncAwait, &[values[0]]).unwrap())),
            TaskOp::GuardHeld => Some(Val::Scalar(self.e.fb.ins().icmp_imm_s(IntCC::NotEqual, values[0], 0))),
            TaskOp::SemaphoreNew | TaskOp::BarrierNew => {
                let location = self.location(span);
                let rt = if op == TaskOp::SemaphoreNew { Rt::SemaphoreNew } else { Rt::BarrierNew };
                Some(Val::Scalar(self.e.call_rt(rt, &[values[0], location]).unwrap()))
            }
            TaskOp::Acquire => {
                let wait = self.e.fb.ins().iconst(types::I8, 0);
                Some(Val::Scalar(self.e.call_rt(Rt::SemaphoreAcquire, &[values[0], wait]).unwrap()))
            }
            TaskOp::TryAcquire => {
                let only = self.e.fb.ins().iconst(types::I8, 1);
                let permit = self.e.call_rt(Rt::SemaphoreAcquire, &[values[0], only]).unwrap();
                let permit_ty = self.e.cg.types().lookup(&TyKind::Handle(Handle::Permit, Ty::UNIT)).unwrap();
                let out = self.scratch(permit_ty);
                self.e.fb.ins().store(flags(), permit, out, 0);
                let found = self.e.fb.ins().icmp_imm_s(IntCC::NotEqual, permit, 0);
                self.removal_result(place, ty, permit_ty, out, found)
            }
            // The permit moved into a temporary, whose release gives it back.
            TaskOp::PermitRelease => None,
            TaskOp::BarrierWait => Some(Val::Scalar(self.e.call_rt(Rt::BarrierWait, &[values[0]]).unwrap())),
            TaskOp::Close => {
                self.e.call_rt(Rt::ChannelClose, &[values[0]]);
                None
            }
            TaskOp::CopySender => Some(Val::Scalar(self.e.call_rt(Rt::SenderCopy, &[values[0]]).unwrap())),
            TaskOp::Abandoned => {
                let mode = self.e.fb.ins().iconst(types::I8, 2);
                Some(Val::Scalar(self.e.call_rt(Rt::ChannelClosed, &[values[0], mode]).unwrap()))
            }
            TaskOp::IsClosed => {
                let sender = matches!(self.e.cg.types().kind(handle_ty.unwrap()), TyKind::Handle(Handle::Sender, _));
                let sender = self.e.fb.ins().iconst(types::I8, sender as i64);
                Some(Val::Scalar(self.e.call_rt(Rt::ChannelClosed, &[values[0], sender]).unwrap()))
            }
            TaskOp::GroupWait => unreachable!("lowered to a loop"),
            TaskOp::Shared(_) | TaskOp::Atomic(_) | TaskOp::Zeroed => unreachable!("handled above"),
        }
    }

    /// `select`: lays out one runtime `Case` per operation (see the
    /// runtime's `tasks`), waits, and stores the result of the operation
    /// that completed in its case's `out`. A value whose send did not
    /// complete is released. Returns the index of that operation.
    pub(super) fn select(&mut self, cases: &[SelectCase], blocking: bool, span: Span) -> ir::Value {
        const CASE: u32 = 32;
        let data = StackSlotData::new(StackSlotKind::ExplicitSlot, CASE * cases.len() as u32, 3);
        let slot = self.e.fb.create_sized_stack_slot(data);
        let base = self.e.fb.ins().stack_addr(POINTER, slot, 0);
        let mut outs = Vec::new();
        let mut sent = Vec::new();
        for (index, case) in cases.iter().enumerate() {
            let at = (CASE * index as u32) as i32;
            let item = self.e.cg.types().handle_result(self.operand_ty(&case.operands[0]));
            let kind = match case.op {
                Some(TaskOp::Receive) => 0,
                Some(TaskOp::Send) => 1,
                Some(TaskOp::Wait) => 2,
                Some(TaskOp::GroupNext) => 3,
                _ => 4,
            };
            let kind_value = self.e.fb.ins().iconst(types::I8, kind);
            let zero = self.e.fb.ins().iconst(types::I8, 0);
            self.e.fb.ins().store(flags(), kind_value, base, at);
            self.e.fb.ins().store(flags(), zero, base, at + 1);
            let first = self.scalar(&case.operands[0]);
            if kind == 4 {
                let null = self.e.fb.ins().iconst(POINTER, 0);
                self.e.fb.ins().store(flags(), null, base, at + 8);
                self.e.fb.ins().store(flags(), first, base, at + 24);
                continue;
            }
            self.e.fb.ins().store(flags(), first, base, at + 8);
            let value = if kind == 1 {
                let value = self.operand(&case.operands[1]);
                if case.retain {
                    self.e.refcount(item, value, false);
                }
                let source = self.copy_to_stack(item, value);
                if self.e.cg.types().needs_drop(item) {
                    sent.push((index, source, item));
                }
                source
            } else {
                let out = self.scratch(item);
                outs.push((index, case.out, kind, item, out));
                out
            };
            self.e.fb.ins().store(flags(), value, base, at + 16);
        }
        let count = self.e.fb.ins().iconst(POINTER, cases.len() as i64);
        let blocking = self.e.fb.ins().iconst(types::I8, blocking as i64);
        let location = self.location(span);
        let won = self.e.call_rt(Rt::Select, &[base, count, blocking, location]).unwrap();
        for (index, out, kind, item, scratch) in outs {
            let (this, next) = (self.e.fb.create_block(), self.e.fb.create_block());
            let is_this = self.e.fb.ins().icmp_imm_s(IntCC::Equal, won, index as i64);
            self.e.fb.ins().brif(is_this, this, &[], next, &[]);
            self.e.fb.switch_to_block(this);
            let place = Place::local(out);
            if kind == 2 {
                let value = self.e.value_at(scratch, item);
                self.write(&place, value);
            } else {
                let found = self.e.fb.ins().load(types::I8, flags(), base, (CASE * index as u32 + 1) as i32);
                let option = self.place_ty(&place);
                if let Some(value) = self.removal_result(&place, option, item, scratch, found) {
                    self.write(&place, value);
                }
            }
            self.e.fb.ins().jump(next, &[]);
            self.e.fb.switch_to_block(next);
        }
        for (index, source, item) in sent {
            let (lost, next) = (self.e.fb.create_block(), self.e.fb.create_block());
            let is_this = self.e.fb.ins().icmp_imm_s(IntCC::Equal, won, index as i64);
            self.e.fb.ins().brif(is_this, next, &[], lost, &[]);
            self.e.fb.switch_to_block(lost);
            let value = self.e.value_at(source, item);
            self.e.refcount(item, value, true);
            self.e.fb.ins().jump(next, &[]);
            self.e.fb.switch_to_block(next);
        }
        won
    }
}
