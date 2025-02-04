use crate::{
    ast::{self, Loc},
    passes::Pass,
    utils::Binding,
};
use itertools::Itertools;
use std::collections::HashMap;

#[derive(Default)]
/// Eliminate the uses of bundles in the component signatures and "splat" the
/// bundle range access syntax.
/// For each component that mentions bundles in its signature, we generate
/// explicit ports for each index in the bundle.
/// For each invocation of such components, we replace the bundle ports
/// ([ast::Port::Bundle] or [ast::Port::InvBundle]) with explicit ports.
pub struct BundleElim {
    /// Mapping from name of a monomorphized instance and bundle name to the
    /// ports generated by the monomorphization of the bundle.
    sig_bundle_map: HashMap<(ast::Id, ast::Id), Vec<ast::Id>>,
    /// Mapping from names of instances to components
    inst_map: HashMap<ast::Id, ast::Id>,
    /// Mapping from invocations to instances
    inv_map: HashMap<ast::Id, ast::Id>,
}

impl BundleElim {
    fn sig_from_invoke(&self, invoke: ast::Id) -> ast::Id {
        let inst = self.inv_map.get(&invoke).unwrap_or_else(|| {
            unreachable!("No instance for invocation `{invoke}'")
        });
        *self.inst_map.get(inst).unwrap_or_else(|| {
            unreachable!("No component for instance `{inst}'")
        })
    }

    /// Compile bundles mentioned in the signature of a component:
    /// - IO bundles are moved into the component body.
    /// - Input bundles generate assignments from the bundle to the port
    /// - Output bundles generate assignments from the port to the bundles
    /// Returns the ports generated from this process and whether the generated ports are inputs.
    fn compile_sig_port(
        p: ast::Bundle,
        is_input: bool,
        pre_cmds: &mut Vec<ast::Command>,
        post_cmds: &mut Vec<ast::Command>,
    ) -> (Vec<Loc<ast::PortDef>>, bool) {
        // Add bundle to the top-level commands
        pre_cmds.push(p.clone().into());
        let ast::Bundle {
            name: bundle_name,
            typ:
                ast::BundleType {
                    idx,
                    len,
                    liveness,
                    bitwidth,
                },
        } = p;
        let len: u64 = len.take().try_into().unwrap();
        // For each index in the bundle, generate a corresponding port
        let ports = (0..len)
            .map(|i| {
                let bind =
                    Binding::new(vec![(*idx.inner(), ast::Expr::concrete(i))]);
                let liveness =
                    liveness.clone().take().resolve_exprs(&bind).into();
                // Name of the new port is the bundle name with the index appended
                let name = Loc::unknown(ast::Id::from(format!(
                    "{}_{i}",
                    bundle_name.clone()
                )));
                // Generate connection associated with this bundle port's creation.
                let this_port = ast::Port::This(name.clone()).into();
                let bundle_port = ast::Port::bundle(
                    bundle_name.clone(),
                    Loc::unknown(ast::Expr::concrete(i).into()),
                )
                .into();
                // Generate assignment for the bundle
                if is_input {
                    // bundle{i} = this.p
                    pre_cmds.push(ast::Command::Connect(ast::Connect::new(
                        bundle_port,
                        this_port,
                        None,
                    )))
                } else {
                    // this.p = bundle{i}
                    post_cmds.push(ast::Command::Connect(ast::Connect::new(
                        this_port,
                        bundle_port,
                        None,
                    )))
                };
                let port = ast::PortDef::Port {
                    name,
                    liveness,
                    bitwidth: bitwidth.clone(),
                };
                port.into()
            })
            .collect_vec();
        (ports, is_input)
    }

    /// Transform the signature of a monomorphized component and generate any assignments needed to
    /// implement the bundles mentioned in the signature.
    fn sig(
        &mut self,
        sig: ast::Signature,
    ) -> (ast::Signature, Vec<ast::Command>, Vec<ast::Command>) {
        // To add before and after the body
        let (mut pre_cmds, mut post_cmds) = (vec![], vec![]);
        let name = *sig.name;
        // Generate ports for each bundle
        let sig = sig.replace_ports(&mut |p, is_input| {
            let pos = p.pos();
            match p.take() {
                p @ ast::PortDef::Port { .. } => {
                    (vec![Loc::new(p, pos)], is_input)
                }
                ast::PortDef::Bundle(b) => {
                    let b_name = *b.name;
                    let (ports, is_input) = Self::compile_sig_port(
                        b,
                        is_input,
                        &mut pre_cmds,
                        &mut post_cmds,
                    );
                    // Add the transformed signature to the bundle map.
                    self.sig_bundle_map.insert(
                        (name, b_name),
                        ports
                            .iter()
                            .map(|p| *p.inner().name().inner())
                            .collect(),
                    );
                    (ports, is_input)
                }
            }
        });
        (sig, pre_cmds, post_cmds)
    }

    fn bundle_splat_ports(
        &self,
        comp: ast::Id,
        bundle: ast::Id,
        (start, end): (ast::Expr, ast::Expr),
    ) -> impl Iterator<Item = Loc<ast::Id>> + '_ {
        let renamed =
            &self.sig_bundle_map.get(&(comp, bundle)).unwrap_or_else(|| {
                unreachable!(
                    "Bundle `{}' not found in component `{}'",
                    bundle, comp
                )
            });
        let start: u64 = start.try_into().unwrap();
        let end: u64 = end.try_into().unwrap();
        (start..end).map(|i| Loc::unknown(renamed[i as usize]))
    }

    fn port(&self, p: Loc<ast::Port>) -> Vec<Loc<ast::Port>> {
        let pos = p.pos();
        match p.take() {
            ast::Port::Bundle { name, access } => {
                // We don't need to rewrite index accesses on signature bundles
                // because they are going to be bound in the body.
                if matches!(access.inner(), ast::Access::Index(_)) {
                    return vec![Loc::new(
                        ast::Port::bundle(name, access),
                        pos,
                    )];
                }
                let ast::Access::Range { start, end } = access.take() else {
                    unreachable!();
                };

                // This is a locally bound bundle
                let s: u64 = start.try_into().unwrap();
                let e: u64 = end.try_into().unwrap();
                (s..e)
                    .map(|idx| {
                        ast::Port::bundle(
                            name.clone(),
                            Loc::unknown(ast::Expr::concrete(idx).into()),
                        )
                        .into()
                    })
                    .collect_vec()
            }
            ast::Port::InvBundle {
                invoke,
                port,
                access,
            } => {
                if let ast::Access::Index(idx) = access.inner() {
                    let comp = self.sig_from_invoke(invoke.copy());
                    let ports = &self
                        .sig_bundle_map
                        .get(&(comp, port.copy()))
                        .unwrap_or_else(|| {
                            unreachable!(
                                "Bundle `{}' not found in component `{}'",
                                port, invoke
                            )
                        });
                    let conc: u64 = idx.try_into().unwrap();
                    let port = ports[conc as usize];
                    return vec![Loc::new(
                        ast::Port::inv_port(invoke, port.into()),
                        pos,
                    )];
                }
                let ast::Access::Range { start, end } = access.take() else {
                    unreachable!();
                };

                self.bundle_splat_ports(
                    self.sig_from_invoke(*invoke),
                    *port,
                    (start, end),
                )
                .map(|name| {
                    ast::Port::InvPort {
                        invoke: invoke.clone(),
                        name,
                    }
                    .into()
                })
                .collect_vec()
            }
            p => vec![Loc::new(p, pos)],
        }
    }

    fn commands(&mut self, cmds: Vec<ast::Command>) -> Vec<ast::Command> {
        cmds.into_iter()
            .flat_map(|cmd| match cmd {
                ast::Command::Instance(ast::Instance {
                    ref name,
                    ref component,
                    ..
                }) => {
                    // Add instance -> component mapping
                    self.inst_map.insert(**name, **component);
                    vec![cmd]
                }
                ast::Command::Invoke(mut inv) => {
                    // Add invoke -> instance mapping
                    self.inv_map.insert(*inv.name, *inv.instance);
                    if let Some(ports) = inv.ports {
                        inv.ports = Some(
                            ports
                                .into_iter()
                                .flat_map(|p| self.port(p))
                                .collect_vec(),
                        );
                    }
                    vec![inv.into()]
                }
                ast::Command::Connect(con) => {
                    // Rewrite the ports
                    let dst = self.port(con.dst);
                    let src = self.port(con.src);
                    assert!(
                        src.len() == dst.len(),
                        "src and dst bundles produced different number of ports"
                    );
                    src.into_iter()
                        .zip(dst.into_iter())
                        .map(|(src, dst)| {
                            ast::Connect {
                                dst,
                                src,
                                guard: con.guard.clone(),
                            }
                            .into()
                        })
                        .collect_vec()
                }
                ast::Command::ForLoop(ast::ForLoop {
                    idx,
                    start,
                    end,
                    body,
                }) => {
                    let body = self.commands(body);
                    vec![ast::ForLoop {
                        idx,
                        start,
                        end,
                        body,
                    }
                    .into()]
                }
                ast::Command::If(ast::If { cond, then, alt }) => {
                    let then = self.commands(then);
                    let alt = self.commands(alt);
                    vec![ast::If { cond, then, alt }.into()]
                }
                c @ (ast::Command::Bundle(_) | ast::Command::Fact(_)) => {
                    vec![c]
                }
            })
            .collect_vec()
    }

    /// Tranverse the component and eliminate bundles.
    fn component(&mut self, comp: ast::Component) -> ast::Component {
        let (sig, mut pre_cmds, post_cmds) = self.sig(comp.sig);
        let body = self.commands(comp.body);
        pre_cmds.extend(body);
        pre_cmds.extend(post_cmds);
        ast::Component {
            sig,
            body: pre_cmds,
            ..comp
        }
    }
}

impl Pass for BundleElim {
    /// Monomorphize the program by generate a component for each parameter of each instance.
    fn transform(old_ns: ast::Namespace) -> ast::Namespace {
        let mut pass = Self::default();
        let mut ns = ast::Namespace {
            components: Vec::new(),
            ..old_ns
        };

        // For each parameter of each instance, generate a new component
        for comp in old_ns.components {
            ns.components.push(pass.component(comp));
        }
        ns
    }
}
