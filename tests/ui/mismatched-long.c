// A pointer is not an integer, so it cannot be passed where a `long int` is
// expected -- even though an `int` could.

long int twice(long int value) {
    return value + value;
}

int main() {
    int x = 21;
    return twice(&x);
}
