// The truth table of `||`, which is 1 unless both operands are false.
//@ exit-code: 14

int main() {
    int t = 1;
    int f = 0;
    return (t || t) * 8 + (t || f) * 4 + (f || t) * 2 + (f || f);
}
