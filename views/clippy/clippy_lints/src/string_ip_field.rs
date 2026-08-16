use clippy_utils::diagnostics::span_lint_and_help;
use rustc_hir::FieldDef;
use rustc_lint::{LateContext, LateLintPass};
use rustc_middle::ty;
use rustc_session::declare_lint_pass;

declare_clippy_lint! {
    /// ### What it does
    /// Flags struct fields whose name suggests they hold an IP address but
    /// whose type is `String`.
    ///
    /// ### Why restrict this?
    /// `String` accepts any text — including invalid IP addresses, accidental
    /// whitespace, partial values, and IPv6 strings where an IPv4 was expected.
    /// Parsing the address once at the boundary into `IpAddr`, `Ipv4Addr`, or
    /// `Ipv6Addr` guarantees validity for the rest of the program and makes
    /// the intent explicit at the type level.
    ///
    /// ### Example
    /// ```rust,ignore
    /// struct Server {
    ///     listen_ip: String,
    /// }
    /// ```
    /// Use instead:
    /// ```rust,ignore
    /// use std::net::IpAddr;
    ///
    /// struct Server {
    ///     listen_ip: IpAddr,
    /// }
    /// ```
    #[clippy::version = "1.86.0"]
    pub STRING_IP_FIELD,
    restriction,
    "struct field named like an IP address but typed as `String`"
}

declare_lint_pass!(StringIpField => [STRING_IP_FIELD]);

impl<'tcx> LateLintPass<'tcx> for StringIpField {
    fn check_field_def(&mut self, cx: &LateContext<'tcx>, field: &'tcx FieldDef<'_>) {
        if field.span.from_expansion() {
            return;
        }

        if !field_name_suggests_ip(field.ident.as_str()) {
            return;
        }

        let field_ty = cx.tcx.type_of(field.def_id).instantiate_identity().skip_norm_wip();
        let ty::Adt(adt_def, _) = field_ty.kind() else {
            return;
        };
        if cx.tcx.lang_items().string() != Some(adt_def.did()) {
            return;
        }

        span_lint_and_help(
            cx,
            STRING_IP_FIELD,
            field.ty.span,
            format!(
                "field `{}` looks like an IP address but is typed as `String`",
                field.ident
            ),
            None,
            "use `std::net::IpAddr` (or `Ipv4Addr` / `Ipv6Addr`) so the address is parsed and validated at construction time",
        );
    }
}

fn field_name_suggests_ip(name: &str) -> bool {
    name.split('_').any(|segment| segment.eq_ignore_ascii_case("ip"))
}
