// Reading through a pointer to a local.
//@ exit-code: 42

int main() {
    int x = 42;
    int *p = &x;
    return *p;
}
