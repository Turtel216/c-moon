// The init clause may be an expression instead of a declaration, in which case
// the variable it assigns outlives the loop.
//@ exit-code: 12

int main() {
    int i;
    int total = 0;

    for (i = 0; i < 4; i = i + 1) {
        total = total + 2;
    }

    return total + i;
}
