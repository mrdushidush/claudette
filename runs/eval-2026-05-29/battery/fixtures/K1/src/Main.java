package shop;

public class Main {
    public static void main(String[] args) {
        TaxCalculator tc = new TaxCalculator(0.08);
        InvoiceService svc = new InvoiceService(tc);
        System.out.println(new PriceFormatter().format(svc.computeTotal(100.0)));
    }
}
