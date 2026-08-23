// The cancellation holds however the pair is stacked, through a second level
// of indirection, and on the left of an assignment.
//@ exit-code: 30

int main() {
    int x = 5;
    int *p = &x;
    int *twice = &*&*p;
    int **pp = &p;
    int *through_pp = &**pp;

    // Writing through a cancelled pair writes the original object.
    *&*twice = 10;

    long int total = 0;
    long int *lp = &*&total;
    *lp = *twice + *through_pp + x;

    return (int)*lp;
}
