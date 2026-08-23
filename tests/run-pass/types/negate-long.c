// Negating a `long int` negates all 64 bits of it: the value below does not
// fit in an `int`, so negating it at that width would lose the top ones.
//@ exit-code: 8

int main() {
    long int big = 4000000000;
    long int negated = -big;

    return -negated / 500000000;
}
