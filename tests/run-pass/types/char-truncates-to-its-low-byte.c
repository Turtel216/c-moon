// Converting an `int` to a `char` keeps the low eight bits, whether the
// conversion is written as an assignment or as a cast.
//@ exit-code: 65

int main() {
    char assigned = 321;
    char cast = (char) 833;
    if (assigned != cast) {
        return 1;
    }
    return assigned;
}
