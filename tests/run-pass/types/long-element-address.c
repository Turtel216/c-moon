// An element address scales its index by the element's own size: the eight
// bytes of a `long int` here, where an `int` would take four.
//@ exit-code: 2

int main() {
    long int values[3];
    values[0] = 1;
    values[1] = 2;
    values[2] = 4000000000;
    int i = 2;

    long int *second = &values[1];
    long int *third = &values[i];

    return (*second == 2) + (*third == 4000000000);
}
