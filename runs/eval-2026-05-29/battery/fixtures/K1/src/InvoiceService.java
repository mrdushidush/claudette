package shop;

/** Builds invoices. Uses TaxCalculator but does not define computeTax itself. */
public class InvoiceService {
    private final TaxCalculator tax;

    public InvoiceService(TaxCalculator tax) {
        this.tax = tax;
    }

    /** Decoy: similarly named, but this is the grand TOTAL, not the tax. */
    public double computeTotal(double subtotal) {
        return subtotal + tax.computeTax(subtotal);
    }
}
