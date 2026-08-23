// Negation composes with itself, with a call, and with a subtraction that
// borrows its sign from the operand next to it.
//@ exit-code: 3

int negated(int value) {
    return -value;
}

int main() {
    int twice = - -7;
    int called = -negated(7);
    int borrowed = 0 - -3;

    return (twice == 7) + (called == 7) + (borrowed == 3);
}
