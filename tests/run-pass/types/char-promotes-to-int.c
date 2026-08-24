// C promotes a `char` operand to `int` before any arithmetic, so `b - a` is
// a subtraction of two `int`s and has type `int`.
//@ exit-code: 1

int main() {
    char a = 'a';
    char b = 'b';
    return b - a;
}
