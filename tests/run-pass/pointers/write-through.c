// A store through a pointer is visible through the original variable, so the
// variable cannot be kept in a register across the store.
//@ exit-code: 55

int main() {
    int x = 10;
    int *p = &x;
    *p = 55;
    return x;
}
