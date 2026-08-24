// Either operand of a logical operator may itself be one, so the left
// operand's own branching decides where the test that follows it sits.
//@ exit-code: 3

int main() {
    int a = 1;
    int b = 0;
    int c = 2;

    // The first group is false and the second true, so the `||` is 1.
    int grouped = (a && b) || (c && a);
    // `!(a || b)` is 0 and `!(a && b)` is 1.
    int negated = !(a || b) + !(a && b);

    return grouped * 2 + negated;
}
