// The truth table of `&&`, which is 1 only when both operands are true.
//@ exit-code: 8

int main() {
    int t = 1;
    int f = 0;
    return (t && t) * 8 + (t && f) * 4 + (f && t) * 2 + (f && f);
}
