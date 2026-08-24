// `!` binds tighter than `&&`, `&&` tighter than `||`, and a comparison
// tighter than any of them. Every line below is written so that any other
// grouping would give a different answer.
//@ exit-code: 5

int main() {
    int t = 1;
    int f = 0;

    // `t || (f && f)` is 1; `(t || f) && f` would be 0.
    int and_binds_tighter = t || f && f;
    // `(!f) && f` is 0; `!(f && f)` would be 1.
    int not_binds_tighter = !f && f;
    // `2 && (3 == 3)` is 1; `(2 && 3) == 3` would be 0.
    int comparison_binds_tighter = 2 && 3 == 3;

    return and_binds_tighter * 4 + not_binds_tighter * 2 + comparison_binds_tighter;
}
