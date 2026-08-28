// The other direction across the link boundary: a GCC-compiled function calls
// one this compiler emitted, which only resolves because a definition is
// given external linkage.
//@ aux-build: callback.c
//@ exit-code: 20

extern int twice_doubled(int x);

int doubled(int x) {
    return x * 2;
}

int main() {
    return twice_doubled(5);
}
