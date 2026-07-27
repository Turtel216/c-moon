// A pointer passed as an argument and dereferenced by the callee.
//@ exit-code: 99

int deref(int *p) {
    return *p;
}

int main() {
    int x = 99;
    return deref(&x);
}
