// `!=` yields 1 for differing operands.
//@ exit-code: 1

int main() {
    int a = 2;
    int b = 3;
    return a != b;
}
