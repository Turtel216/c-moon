// `a` must survive the second call, so it cannot live in a caller-saved
// register that the callee is free to destroy.
//@ exit-code: 14

int f(int x) {
    int y = x + 1;
    int z = y + 1;
    return z * 2;
}

int main() {
    int a = f(1);
    int b = f(2);
    return a + b;
}
