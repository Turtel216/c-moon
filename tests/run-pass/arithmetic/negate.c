// Unary negation, of a literal, of a variable, and of a computed value.
//@ exit-code: 42

int main() {
    int five = -5;
    int back = -five;
    int product = -(back * 3);

    return back * 10 - product - 23;
}
