use itertools::Itertools;

use crate::ir::{self, Ctx};
use crate::ir_visitor::{Action, Visitor};
use crate::utils::GPosIdx;

#[derive(Default)]
/// Filament's core interval checking algorithm. At a high-level it ensures that:
/// 1. All delays are well-formed
/// 2. Ports are connected for as long as expected
///
/// In order to ensure that delays are well-formed, we need to ensure that:
/// * Invocations provide events that trigger less often that expected by the
///   delay of the invoked component.
/// * The availability of bundle signals is less than the delay
/// * Shared instances are live for shorter duration than the delay
///
/// Like [super::TypeCheck], this pass simply generates all the assertions that
/// enforce the above constraints.
/// It is the job of a latter pass to ensure that the assertions are discharged.
pub struct IntervalCheck;

impl IntervalCheck {
    /// Constraints to ensure that the range is well-formed, i.e., the end of
    /// the range is strictly greater than the start.
    fn range_wf(
        &mut self,
        range: &ir::Range,
        loc: GPosIdx,
        comp: &mut ir::Component,
    ) -> Option<ir::Command> {
        let &ir::Range { start, end } = range;
        let prop = end.gt(start, comp);
        let reason = comp
            .add(ir::Reason::well_formed_interval(loc, (start, end)).into());
        comp.assert(prop, reason)
    }

    /// Check that event delays are greater than zero
    fn delay_wf(
        &mut self,
        event: ir::EventIdx,
        comp: &mut ir::Component,
    ) -> Option<ir::Command> {
        let zero = comp.num(0).into();
        let ir::Event { delay, info, .. } = &comp[event];
        let ir::Info::Event { delay_loc, .. } = comp[*info] else {
            unreachable!("expected event info")
        };
        let prop = delay.clone().gt(zero, comp);
        let reason = comp.add(
            ir::Reason::misc("delay must be greater than zero", delay_loc)
                .into(),
        );
        comp.assert(prop, reason)
    }

    /// Proposition that ensures that the given parameter is in range
    fn in_range(live: &ir::Liveness, comp: &mut ir::Component) -> ir::PropIdx {
        let &ir::Liveness { idx, len, .. } = live;
        let zero = comp.num(0);
        let idx = idx.expr(comp);
        let lo = idx.gte(zero, comp);
        let hi = idx.lt(len, comp);
        lo.and(hi, comp)
    }
}

impl Visitor for IntervalCheck {
    fn start(&mut self, comp: &mut ir::Component) -> Action {
        // Ensure that delays are greater than zero
        let mut cmds: Vec<ir::Command> =
            Vec::with_capacity(comp.ports().len() + comp.events().len());
        for idx in comp.events().idx_iter() {
            let ev = &comp[idx];
            if ev.owner.is_sig() {
                cmds.extend(self.delay_wf(idx, comp));
            }
        }

        // For each bundle, add an assertion to ensure that availability of the
        // bundle signal is less than the delay.
        // Extract the ranges first because we cannot borrow comp mutably before this.
        let ranges = comp
            .ports()
            .iter()
            .filter_map(|(_, p)| {
                // Ignore ports on invokes
                if !matches!(p.owner, ir::PortOwner::Inv { .. }) {
                    Some((p.live.clone(), p.info))
                } else {
                    None
                }
            })
            .collect_vec();

        for (live, info) in ranges {
            let ir::Info::Port { live_loc, .. } = comp[info] else {
                unreachable!("expected port info")
            };
            let range = live.range;
            // Require that the range is well-formed
            cmds.extend(self.range_wf(&range, live_loc, comp));

            // We only constraint the event mentioned in the start of the range.
            let st_ev = comp[range.start].event;
            let len = range.end.sub(range.start, comp);
            let ev = &comp[st_ev];
            let delay = ev.delay.clone();
            let ir::Info::Event { delay_loc, .. } = comp[ev.info] else {
                unreachable!("expected event info")
            };
            let param = comp.get(live.idx);
            let ir::Info::Param { bind_loc, .. } = comp[param.info] else {
                unreachable!("expected param info")
            };
            let zero = comp.num(0);
            let reason = comp.add(
                ir::Reason::bundle_delay(
                    delay_loc,
                    live_loc,
                    len.clone(),
                    bind_loc,
                    (zero, live.len),
                )
                .into(),
            );
            let prop = comp
                .add(ir::Prop::TimeSubCmp(ir::CmpOp::gte(delay.clone(), len)));
            cmds.extend(comp.assert(prop, reason));
        }

        Action::AddBefore(cmds)
    }

    /// For each event binding, we add the constraint that the events uses as arguments
    /// are triggered less often than the delay of the invoked component.
    fn event_binding(
        &mut self,
        eb: &mut ir::EventBind,
        comp: &mut ir::Component,
    ) -> Action {
        let ir::EventBind { event, arg, info } = &eb;
        let ir::Info::EventBind { bind_loc } = comp[*info] else {
            unreachable!("expected event bind info")
        };

        let inv_ev = &comp[*event];
        let inv_delay = inv_ev.delay.clone();
        let ir::Info::Event { delay_loc: inv_del_loc, .. } = comp[inv_ev.info] else {
            unreachable!("expected event info")
        };

        let this_ev = &comp[comp[*arg].event];
        let this_delay = this_ev.delay.clone();
        let ir::Info::Event { delay_loc: ev_del_loc, .. } = comp[this_ev.info] else {
            unreachable!("expected event info")
        };

        let reason = comp.add(
            ir::Reason::event_trig(
                inv_del_loc,
                inv_delay.clone(),
                ev_del_loc,
                this_delay.clone(),
                bind_loc,
            )
            .into(),
        );

        // Ensure that this event's delay is greater than invoked component's event's delay.
        let prop = comp
            .add(ir::Prop::TimeSubCmp(ir::CmpOp::gte(this_delay, inv_delay)));
        let fact = comp.assert(prop, reason);
        if let Some(c) = fact {
            Action::AddBefore(vec![c])
        } else {
            Action::Continue
        }
    }

    fn connect(
        &mut self,
        con: &mut ir::Connect,
        comp: &mut ir::Component,
    ) -> Action {
        let ir::Connect { src, dst, info } = con;
        let src_t = src.bundle_typ(comp);
        let dst_t = dst.bundle_typ(comp);
        let in_range = Self::in_range(&dst_t, comp)
            .and(Self::in_range(&src_t, comp), comp);

        // Substitute the parameter used in source with that in dst
        let binding = [(dst_t.idx, src_t.idx.expr(comp))];
        let dst_range =
            ir::Subst::new(dst_t.range, &ir::Bind::new(&binding)).apply(comp);

        // Assuming that lengths are equal
        let pre_req = src_t.len.equal(dst_t.len, comp).and(in_range, comp);
        let contains = src_t
            .range
            .start
            .lte(dst_range.start, comp)
            .and(src_t.range.end.gte(dst_range.end, comp), comp);

        let ir::Info::Connect { dst_loc, src_loc } = comp.get(*info) else {
            unreachable!("Expected connect info")
        };
        let reason = comp.add(
            ir::Reason::liveness(*dst_loc, *src_loc, dst_range, src_t.range)
                .into(),
        );

        let prop = pre_req.implies(contains, comp);
        let f = comp.assert(prop, reason);
        if let Some(c) = f {
            Action::AddBefore(vec![c])
        } else {
            Action::Continue
        }
    }
}
