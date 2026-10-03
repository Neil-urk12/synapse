use super::ASTParser;
use std::path::Path;

#[test]
fn class_headers_keep_outer_ownership_but_class_bodies_are_unowned() {
    let parsed = ASTParser::parse_file(
        Path::new("x.py"),
        "def outer():\n class Nested(base()):\n  inside()\n outside()\n",
    );
    let owners: Vec<_> = parsed
        .calls
        .iter()
        .map(|c| (c.name.as_str(), c.owner_symbol_id.as_deref()))
        .collect();
    assert_eq!(
        owners,
        [
            ("base", Some("x.py::outer")),
            ("inside", None),
            ("outside", Some("x.py::outer"))
        ]
    );
}

#[test]
fn anonymous_callable_bodies_do_not_borrow_the_outer_owner() {
    for (path, source, owner) in [
        (
            "x.rs",
            "fn outer(){let callback=|| inside(); outside();}",
            "x.rs::outer",
        ),
        (
            "x.js",
            "function outer(){const callback=()=>inside(); outside();}",
            "x.js::outer",
        ),
        (
            "x.go",
            "package main\nfunc outer(){callback:=func(){inside()}; outside()}\n",
            "x.go::outer",
        ),
        (
            "x.py",
            "def outer():\n callback=lambda: inside()\n outside()\n",
            "x.py::outer",
        ),
        (
            "x.cpp",
            "void outer(){auto callback=[](){inside();}; outside();}",
            "x.cpp::outer",
        ),
        (
            "x.java",
            "class Demo{void outer(){Runnable callback=()->inside(); outside();}}",
            "x.java::Demo::outer",
        ),
        (
            "x.kt",
            "fun outer(){val callback={ inside() }; outside()}",
            "x.kt::outer",
        ),
        (
            "x.rb",
            "def outer(); [1].each { inside(); }; outside(); end",
            "x.rb::outer",
        ),
        (
            "x.php",
            "<?php function outer(){ $callback=fn()=>inside(); outside(); }",
            "x.php::outer",
        ),
        (
            "x.swift",
            "func outer(){let callback={ inside() }; outside()}",
            "x.swift::outer",
        ),
    ] {
        let parsed = ASTParser::parse_file(Path::new(path), source);
        let inside = parsed
            .calls
            .iter()
            .find(|call| call.name == "inside")
            .unwrap();
        let outside = parsed
            .calls
            .iter()
            .find(|call| call.name == "outside")
            .unwrap();
        assert_eq!(inside.owner_symbol_id, None, "{path}: {parsed:?}");
        assert_eq!(
            outside.owner_symbol_id.as_deref(),
            Some(owner),
            "{path}: {parsed:?}"
        );
    }
}

#[test]
fn callable_ownership_is_recorded_across_all_parsers() {
    let fixtures = [
        (
            "x.rs",
            "fn target(){} fn actual(){target();} fn idle(){}",
            "x.rs::actual",
        ),
        (
            "x.js",
            "function target(){} class Demo { actual(){ target(); } idle(){} }",
            "x.js::Demo::actual",
        ),
        (
            "x.ts",
            "function target():void{} class Demo { actual():void{ target(); } }",
            "x.ts::Demo::actual",
        ),
        (
            "x.go",
            "package main\nfunc target(){}\nfunc actual(){target()}\n",
            "x.go::actual",
        ),
        (
            "x.py",
            "def target(): pass\nclass Demo:\n def actual(self): target()\n def idle(self): pass\n",
            "x.py::Demo::actual",
        ),
        (
            "x.c",
            "void target(){} void actual(){target();} void idle(){}",
            "x.c::actual",
        ),
        (
            "x.cpp",
            "void target(){} class Demo { void actual(){target();} };",
            "x.cpp::Demo::actual",
        ),
        (
            "x.java",
            "class Demo { void target(){} void actual(){target();} void idle(){} }",
            "x.java::Demo::actual",
        ),
        (
            "x.kt",
            "fun target(){}\nclass Demo { fun actual(){target()} }",
            "x.kt::Demo::actual",
        ),
        (
            "x.rb",
            "def target(); end\nclass Demo\n def actual(); target(); end\nend",
            "x.rb::Demo::actual",
        ),
        (
            "x.php",
            "<?php function target(){} class Demo { function actual(){target();} }",
            "x.php::Demo::actual",
        ),
        (
            "x.swift",
            "func target(){}\nclass Demo { func actual(){target()} }",
            "x.swift::Demo::actual",
        ),
    ];
    for (path, source, owner) in fixtures {
        let parsed = ASTParser::parse_file(Path::new(path), source);
        let calls: Vec<_> = parsed.calls.iter().filter(|c| c.name == "target").collect();
        assert_eq!(calls.len(), 1, "{path}: {parsed:?}");
        assert_eq!(calls[0].owner_symbol_id.as_deref(), Some(owner), "{path}");
    }
}

#[test]
fn nested_function_calls_belong_only_to_the_nearest_body() {
    let parsed = ASTParser::parse_file(
        Path::new("x.py"),
        "def outer():\n def inner():\n  target()\n other()\ntop()\n",
    );
    let owners: Vec<_> = parsed
        .calls
        .iter()
        .map(|c| (c.name.as_str(), c.owner_symbol_id.as_deref()))
        .collect();
    assert_eq!(
        owners,
        [
            ("target", Some("x.py::outer::inner")),
            ("other", Some("x.py::outer")),
            ("top", None)
        ]
    );
}

#[test]
fn calls_in_defaults_are_outside_the_declared_body() {
    let parsed = ASTParser::parse_file(
        Path::new("x.py"),
        "def outer():\n def inner(value=default()):\n  inside()\n",
    );
    let owners: Vec<_> = parsed
        .calls
        .iter()
        .map(|c| (c.name.as_str(), c.owner_symbol_id.as_deref()))
        .collect();
    assert_eq!(
        owners,
        [
            ("default", Some("x.py::outer")),
            ("inside", Some("x.py::outer::inner"))
        ]
    );
}
