// The result of `fib(n - 1)` is live across the `fib(n - 2)` call.
//@ exit-code: 55

int fib(int n) {
    if (n < 2) { return n; }
    return fib(n - 1) + fib(n - 2);
}

int main() {
    return fib(10);
}
