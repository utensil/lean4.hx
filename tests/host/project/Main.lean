example (n : Nat) : n = n := by
  rfl

example (a b c d e f g h : Nat) :
    a + b + c + d + e + f + g + h = h + g + f + e + d + c + b + a := by
  omega

example (p q : Prop) (hp : p) (hq : q) : p ∧ q := by
  constructor
#check Nat
/-- info: 41 -/
#guard_msgs in
#eval 42
