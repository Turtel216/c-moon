// A function may be declared before it is defined, with or without `extern`,
// and the definition further down the file is the one that resolves it.
//@ exit-code: 30

extern int quadruple(int x);
int negate(int x);

int main() {
    return quadruple(5) + negate(-10);
}

int quadruple(int x) {
    return x * 4;
}

int negate(int x) {
    return -x;
}
