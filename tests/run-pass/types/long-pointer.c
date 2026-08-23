// A pointer to `long int` reads and writes all eight bytes of what it points
// at.
//@ exit-code: 5

int main() {
    long int x = 5000000000;
    long int *p = &x;
    *p = *p + 1;
    return x / 1000000000;
}
