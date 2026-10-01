-- Synthetic reference data. Public airport/airline facts with invented ids; "Iberia Charter"
-- is made up, to have two airlines sharing one IATA code.
INSERT INTO Airport (id,name,iata,icao,city,country,countryCode,timeZoneIdentifier,latitude,longitude,relevance,created,lastUpdated) VALUES
 ('a0000000-0000-4000-8000-000000000001','London Heathrow','LHR','EGLL','London','United Kingdom','GB','Europe/London',51.4706,-0.4619,100,0,0),
 ('a0000000-0000-4000-8000-000000000002','Adolfo Suárez Madrid-Barajas','MAD','LEMD','Madrid','Spain','ES','Europe/Madrid',40.4983,-3.5676,100,0,0),
 ('a0000000-0000-4000-8000-000000000003','Benito Juárez International','MEX','MMMX','Mexico City','Mexico','MX','America/Mexico_City',19.4363,-99.0721,100,0,0),
 ('a0000000-0000-4000-8000-000000000004','Miguel Hidalgo y Costilla International','GDL','MMGL','Guadalajara','Mexico','MX','America/Mexico_City',20.5218,-103.3111,100,0,0),
 ('a0000000-0000-4000-8000-000000000005','Querétaro Intercontinental','QRO','MMQT','Querétaro','Mexico','MX','America/Mexico_City',20.6173,-100.1857,10,0,0),
 ('a0000000-0000-4000-8000-000000000006','Düsseldorf Airport','DUS','EDDL','Düsseldorf','Germany','DE','Europe/Berlin',51.2895,6.7668,100,0,0);

INSERT INTO Airline (id,name,iata,icao,alliance,relevance,created,lastUpdated) VALUES
 ('b0000000-0000-4000-8000-000000000001','Iberia','IB','IBE','Oneworld',100,0,0),
 ('b0000000-0000-4000-8000-000000000002','Iberia Charter','IB','IBC',NULL,50,0,0),
 ('b0000000-0000-4000-8000-000000000003','Aeroméxico','AM','AMX','SkyTeam',100,0,0),
 ('b0000000-0000-4000-8000-000000000004','Lufthansa','LH','DLH','Star Alliance',100,0,0);

INSERT INTO AircraftType (id,name,iata,icao,manufacturer,relevance,lastUpdated,created) VALUES
 ('c0000000-0000-4000-8000-000000000001','Airbus A350-900','359','A359','Airbus',100,0,0),
 ('c0000000-0000-4000-8000-000000000002','Airbus A321','321','A321','Airbus',100,0,0);
