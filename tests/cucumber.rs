#![allow(clippy::assertions_on_constants)]

use cucumber_rust::{after, before, cucumber, World};

pub struct MyWorld {
    pub thing: bool,
    pub last_thing: String,
}

impl World for MyWorld {}

impl Default for MyWorld {
    fn default() -> MyWorld {
        MyWorld {
            thing: false,
            last_thing: "".to_owned(),
        }
    }
}

#[cfg(test)]
mod basic {
    use cucumber_rust::steps;
    use regex::Regex;

    steps!(crate::MyWorld => {
        when regex "thing (\\d+) does (.+)" (usize, String) |_world, _sz, _txt, _step| {

        };

        when regex "^test (.*) regex$" |_world, matches, _step| {
            println!("{}", matches[1]);
        };

        given "a thing" |_world, _step| {
            assert!(true);
        };

        given regex "a thing is (.*)" |world, matches, _step| {
            world.last_thing = matches[1].clone();
            assert!(true);
        };

        then regex "it's reverse is (.*)" |world, matches, _step| {
            let rev = world.last_thing.chars().rev().collect::<String>();
            assert_eq!(rev, matches[1]);
        };


        then regex "this (table|docstring) makes sense:" |_world, matches, step| {

            match matches[1].as_str() {
                "table" => {
                    let table = step.table.clone().expect("this step needs a table!");
                    for row in table.rows {
                        assert!(row.len() == 2);
                        assert_eq!(row[0].chars().rev().collect::<String>(), row[1]);
                    };
                },
                "docstring" => {
                    let text = step.docstring.clone().expect("this step needs a docstring!");
                    let captures = Regex::new(r"the reverse of (.*) is (.*)")
                        .unwrap()
                        .captures(&text)
                        .expect("text didn't match the expected format!");
                    assert!(captures.len() == 3);
                    assert_eq!(captures[1].chars().rev().collect::<String>(), captures[2]);
                },
                _ => panic!("only understand table or docstring")
            };

        };

        when "another thing" |_world, _step| {
            panic!();
        };

        when "something goes right" |_world, _step| {
            assert!(true);
        };

        when "something goes wrong" |_world, _step| {
            println!("Something to stdout");
            eprintln!("Something to stderr");
            panic!("This is my custom panic message");
        };

        then "another thing" |_world, _step| {
            assert!(true)
        };

        then "things can also be data tables" |_world, step| {
            let table = step.table().unwrap().clone();

            assert_eq!(table.header, vec!["key", "value"]);

            let expected_keys = table.rows.iter().map(|row| row[0].to_owned()).collect::<Vec<_>>();
            let expected_values = table.rows.iter().map(|row| row[1].to_owned()).collect::<Vec<_>>();

            assert_eq!(expected_keys, vec!["a", "b"]);
            assert_eq!(expected_values, vec!["fizz", "buzz"]);
        };
    });
}

fn before_thing(_step: &cucumber_rust::Scenario) {}

before!(some_before: "@tag2 and @tag3" => |_scenario| {
    println!("{}", "lol");
});

before!(something_great => |_scenario| {

});

after!(after_thing => |_scenario| {

});

fn setup() {}

cucumber! {
    features: "./features",
    world: crate::MyWorld,
    steps: &[
        basic::steps
    ],
    setup: setup,
    before: &[before_thing, some_before, something_great],
    after: &[after_thing]
}
