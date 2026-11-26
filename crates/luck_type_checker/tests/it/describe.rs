use luck_type_checker::{CompletionKind, Description};

use crate::common::{check, checker};

const APP: &str = "\
type Protocol = \"tcp\" | \"udp\"
declare extern type Socket with
    function Send(self, data: string, port: number): ()
end
declare app: {
    net: {
        connect: (host: string, protocol: Protocol?) -> Socket,
    },
}
declare function greet(name: string): string
";

fn described(source: &str, at: &str, before: usize) -> Description {
    let mut checker = checker(&[("main", source)]);
    checker.retain_type_graphs();
    checker.load_definitions("@app", APP).unwrap();
    check(&mut checker, &["main"]).unwrap();
    let offset = u32::try_from(source.find(at).unwrap() + before).unwrap();
    checker.describe("main", offset).expect("something is there")
}

#[test]
fn a_global_function_is_its_type_and_documentation_symbol() {
    let source = "local greeting = greet(\"world\")\n";
    let description = described(source, "greet(", 1);

    assert_eq!(description.ty, "(name: string) -> string");
    assert_eq!(&source[description.span.start as usize..description.span.end as usize], "greet");
    assert_eq!(description.declaration.documentation_symbol.as_deref(), Some("@app/global/greet"));
    assert_eq!(description.signatures.len(), 1);
    let signature = &description.signatures[0];
    assert_eq!(signature.label, "(name: string): string");
    let params: Vec<&str> = signature
        .params
        .iter()
        .map(|span| &signature.label[span.start as usize..span.end as usize])
        .collect();
    assert_eq!(params, ["name: string"]);
}

#[test]
fn a_nested_member_is_where_its_definitions_declare_it() {
    let source = "local socket = app.net.connect(\"host\")\n";
    let description = described(source, "connect", 2);

    let (module, span) = description.declaration.definition.expect("a declaration");
    assert_eq!(module, "@app");
    assert!(APP[span.start as usize..].starts_with("connect:"), "{span:?}");
}

#[test]
fn a_method_call_leaves_out_self() {
    let source = "\
local socket = app.net.connect(\"host\")
socket:Send(\"data\", 1)
";
    let description = described(source, "Send", 1);

    assert_eq!(
        description.declaration.documentation_symbol.as_deref(),
        Some("@app/globaltype/Socket.Send")
    );
    let signature = &description.signatures[0];
    let params: Vec<&str> = signature
        .params
        .iter()
        .map(|span| &signature.label[span.start as usize..span.end as usize])
        .collect();
    assert_eq!(params, ["data: string", "port: number"], "{}", signature.label);
}

#[test]
fn a_type_reference_is_the_type_it_names() {
    let source = "local socket: Socket? = nil\n";
    let description = described(source, "Socket", 1);

    assert_eq!(description.ty, "Socket");
    assert_eq!(
        description.declaration.documentation_symbol.as_deref(),
        Some("@app/globaltype/Socket")
    );
}

#[test]
fn a_local_is_its_type() {
    let source = "\
local count = 1
return count
";
    assert_eq!(described(source, "count", 1).ty, "number");
}

#[test]
fn autocomplete_offers_members_with_their_types_and_declarations() {
    let source = "\
local socket = app.net.connect(\"host\")
socket:
";
    let mut checker = checker(&[("main", source)]);
    checker.retain_type_graphs();
    checker.load_definitions("@app", APP).unwrap();
    check(&mut checker, &["main"]).unwrap();
    let offset = u32::try_from(source.find("socket:").unwrap() + "socket:".len()).unwrap();

    let completions = checker.autocomplete("main", offset);

    let send = completions.iter().find(|completion| completion.name == "Send").expect("Send");
    assert_eq!(send.kind, CompletionKind::Property);
    assert_eq!(send.ty.as_deref(), Some("(self: Socket, data: string, port: number) -> ()"));
    assert_eq!(
        send.declaration.documentation_symbol.as_deref(),
        Some("@app/globaltype/Socket.Send")
    );
}

#[test]
fn a_module_checked_without_its_types_describes_nothing() {
    let source = "local count = 1\n";
    let mut checker = checker(&[("main", source)]);
    check(&mut checker, &["main"]).unwrap();

    assert_eq!(checker.describe("main", 7), None);
}
