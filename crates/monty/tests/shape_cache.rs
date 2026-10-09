use monty::MontyRun;
use monty_types::{CompileOptions, MontyObject};

#[test]
fn instance_attribute_cache_tracks_layouts_and_writes() {
    let code = r#"
class A:
    def __init__(self, x):
        self.x = x

    def method(self):
        return self.x

class B:
    def __init__(self, x):
        self.y = 0
        self.x = x

a = A(1)
b = B(4)
total = 0
for obj in [a, b, a, b]:
    total += obj.x
a.x = 10
total += a.x
object.__setattr__(b, 'x', 20)
total += b.x
total += a.method()
total
"#;
    let mut run = MontyRun::new(code.to_owned(), "shape_cache.py", vec![], CompileOptions::default()).unwrap();
    assert_eq!(run.run_no_limits(vec![]).unwrap(), MontyObject::int(50));
}

#[test]
fn megamorphic_attribute_site_still_uses_normal_lookup() {
    let code = r#"
class Point:
    x = 1

objects = []
for i in range(8):
    obj = Point()
    object.__setattr__(obj, str(i), i)
    objects.append(obj)

total = 0
for round in range(4):
    for obj in objects:
        total += obj.x
    if round == 2:
        objects[5].x = 10
total
"#;
    let mut run = MontyRun::new(code.to_owned(), "shape_cache.py", vec![], CompileOptions::default()).unwrap();
    assert_eq!(run.run_no_limits(vec![]).unwrap(), MontyObject::int(41));
}

#[test]
fn class_fallback_can_become_an_instance_attribute() {
    let code = r#"
class Point:
    x = 1

point = Point()
total = 0
for i in range(8):
    total += point.x
    if i == 3:
        point.x = 2
total
"#;
    let mut run = MontyRun::new(code.to_owned(), "shape_cache.py", vec![], CompileOptions::default()).unwrap();
    assert_eq!(run.run_no_limits(vec![]).unwrap(), MontyObject::int(12));
}
