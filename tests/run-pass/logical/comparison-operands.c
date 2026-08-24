// The operands of a logical operator are usually comparisons, whose 0 or 1
// the operator takes as it stands.
//@ exit-code: 7

int main() {
    int a = 1;
    int b = 5;
    return (a < b && b > 4) * 4 + (a > b || b == 5) * 2 + !(a == b);
}
