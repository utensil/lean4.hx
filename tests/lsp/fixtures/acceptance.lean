import Helper

def answer : Nat := helper

theorem identity (x : Nat) : x = x := by
  exact rfl

-- Keep one diagnostic and a possible quick-fix surface in the fixture.
#check doesNotExist
