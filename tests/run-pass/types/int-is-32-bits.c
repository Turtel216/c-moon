// Assigning a wider value to an `int` keeps its low 32 bits, so a value that
// differs from 2 only above bit 32 arrives as 2.
//@ exit-code: 2

int main() {
    long int big = 4294967298;
    int narrowed = big;
    return narrowed;
}
