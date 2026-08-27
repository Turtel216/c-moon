// A loop body needs no braces, and a `break` or a `continue` is a statement
// like any other in that position.
//@ exit-code: 5

int main() {
    int i = 0;

    while (i < 5) i = i + 1;

    for (int j = 0; j < 3; j = j + 1) continue;

    while (1) break;

    return i;
}
